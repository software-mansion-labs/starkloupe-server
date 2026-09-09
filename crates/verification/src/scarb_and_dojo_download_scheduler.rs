use crate::binary_names::{bucket_arch_folder, installed_marker_relative_path, Tool};
use async_compression::tokio::bufread::GzipDecoder;
use async_tar::Archive;
use aws_sdk_s3::primitives::ByteStream;
use aws_sdk_s3::Client as S3Client;
use futures::StreamExt;
use lazy_regex::regex;
use reqwest::Client;
use semver::Version;
use serde::Deserialize;
use std::env::consts::ARCH;
use std::error::Error;
use std::path::Path;
use tokio::fs as tokio_fs;
use tokio::fs::File;
use tokio::io::{AsyncWriteExt, BufReader};
use tokio::process::Command;
use tokio_util::compat::TokioAsyncReadCompatExt;
use tracing::{debug, info, warn};

// Struct to deserialize GitHub API release response
#[derive(Debug, Deserialize)]
struct Release {
    tag_name: String,
    assets: Vec<Asset>,
}

#[derive(Debug, Deserialize)]
struct Asset {
    browser_download_url: String,
    name: String,
}

/// The most releases GitHub returns in one page; asking for more is capped here.
const RELEASES_PER_PAGE: usize = 100;

/// A repository with more releases than this is not walked to the end. Scarb and
/// Dojo are in the low hundreds, so the cap only guards against an endless walk
/// if the API ever stops honouring `page`.
const MAX_RELEASE_PAGES: usize = 50;

/// Every release of `repo`, newest first.
///
/// The endpoint pages at 30 by default and never returns more than
/// [`RELEASES_PER_PAGE`] at a time, so the pages have to be walked: a single
/// request hides every release older than the newest 30, which is where the
/// versions this scheduler still has to install live.
async fn get_all_releases(repo: &str) -> Result<Vec<Release>, Box<dyn std::error::Error>> {
    let client = Client::new();
    let mut releases: Vec<Release> = Vec::new();

    for page in 1..=MAX_RELEASE_PAGES {
        let url = format!(
            "https://api.github.com/repos/{}/releases?per_page={}&page={}",
            repo, RELEASES_PER_PAGE, page
        );
        let response = client
            .get(&url)
            .header("User-Agent", "rust-app")
            .send()
            .await?
            // Without this a rate-limited 403 deserializes into a confusing
            // "expected a sequence" instead of saying what GitHub answered.
            .error_for_status()?;
        let page_releases: Vec<Release> = response.json().await?;

        // A short page is the last one, so this costs no extra request.
        let is_last_page = page_releases.len() < RELEASES_PER_PAGE;
        releases.extend(page_releases);
        if is_last_page {
            debug!("Fetched {} releases of {}", releases.len(), repo);
            return Ok(releases);
        }
    }

    warn!(
        "Stopped paginating {} releases after {} pages ({} releases); older releases were not considered",
        repo,
        MAX_RELEASE_PAGES,
        releases.len()
    );
    Ok(releases)
}

async fn download_file(url: &str, output_path: &Path) -> Result<(), Box<dyn std::error::Error>> {
    let response = Client::new()
        .get(url)
        .header("User-Agent", "rust-app")
        .send()
        .await?
        .error_for_status()?;

    let mut file = File::create(output_path).await?;
    let mut stream = response.bytes_stream();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk?;
        file.write_all(&chunk).await?;
    }

    Ok(())
}

async fn extract_tar_gz(
    archive_path: &Path,
    output_dir: &Path,
) -> Result<(), Box<dyn std::error::Error>> {
    let file = File::open(archive_path).await?;
    let buf_reader = BufReader::new(file);

    let gzip_decoder = GzipDecoder::new(buf_reader);
    let compat_reader = gzip_decoder.compat();
    let archive = Archive::new(compat_reader);

    archive.unpack(output_dir).await?;
    Ok(())
}

// call scarb and extract the cairo_version
async fn extract_cairo_version(binary_path: &str) -> Result<Version, Box<dyn Error>> {
    let output = Command::new(binary_path).arg("--version").output().await?;

    let scarb_output = String::from_utf8(output.stdout)?;

    let regex = regex!(r"cairo: ([\d\.]+)");
    let version_str = regex
        .captures(&scarb_output)
        .and_then(|caps| caps.get(1))
        .ok_or("Failed to find cairo version in scarb output")?
        .as_str();

    let version = Version::parse(version_str)?;

    Ok(version)
}

/// The suffix of the release asset holding `tool`'s build for this machine.
fn asset_suffix_for_arch(tool: Tool) -> Result<&'static str, Box<dyn Error>> {
    let architecture = ARCH;
    let suffix = match (tool, architecture) {
        (Tool::Sozo, "x86_64") => "linux_amd64.tar.gz",
        (Tool::Sozo, "aarch64") => "darwin_arm64.tar.gz",
        (Tool::Scarb, "x86_64") => "x86_64-unknown-linux-gnu.tar.gz",
        (Tool::Scarb, "aarch64") => "aarch64-apple-darwin.tar.gz",
        _ => {
            return Err(Box::from(format!(
                "Unsupported architecture: {}",
                architecture
            )))
        }
    };
    Ok(suffix)
}

// Parse version from tag name, handling both "v1.8.0" and "sozo/v1.8.1" formats
fn parse_version_from_tag(tag_name: &str) -> String {
    let mut version_str = tag_name.trim();

    if version_str.starts_with("sozo/") {
        version_str = &version_str[5..];
    }
    version_str = version_str.trim_start_matches('v');
    version_str.to_string()
}

pub async fn check_periodically_sozo_updates(
    repo: &str,
    s3_client: &S3Client,
) -> Result<(), Box<dyn Error>> {
    check_periodically_updates(repo, Tool::Sozo, "1.0.12", "/sozo", s3_client).await
}

pub async fn check_periodically_scarb_updates(
    repo: &str,
    s3_client: &S3Client,
) -> Result<(), Box<dyn std::error::Error>> {
    check_periodically_updates(repo, Tool::Scarb, "2.8.5", "/bin/scarb", s3_client).await
}

/// The bucket the binaries are cached in, if one is configured.
///
/// Local runs leave `BINARIES_S3_BUCKET_NAME` empty; there is nothing to cache
/// into then, and an install is none the worse for it.
fn binaries_bucket_name() -> Option<String> {
    std::env::var("BINARIES_S3_BUCKET_NAME")
        .ok()
        .map(|name| name.trim().to_string())
        .filter(|name| !name.is_empty())
}

/// Put a freshly installed file in the binaries bucket, so the next cold start
/// restores it instead of coming back to GitHub for it.
///
/// An object already under this key is overwritten. Several releases of Scarb
/// can ship one Cairo version and so land on one file name, and the newest of
/// them is the one the install left on this machine's disk - leaving the older
/// build in the bucket would have a cold start restore a different binary than
/// the machine that downloaded it runs.
///
/// Best effort: the file is already on this machine's disk, so a bucket that is
/// unreachable, unwritable or unconfigured costs the next cold start a download
/// and nothing else. It must not fail the install.
async fn cache_in_bucket(s3_client: &S3Client, tool: Tool, file_name: &str, local_path: &str) {
    let Some(bucket_name) = binaries_bucket_name() else {
        debug!("No binaries bucket configured, not caching {}", file_name);
        return;
    };
    let arch_folder = match bucket_arch_folder() {
        Ok(folder) => folder,
        Err(err) => {
            warn!("Not caching {}: {}", file_name, err);
            return;
        }
    };
    let key = tool.bucket_key(arch_folder, file_name);

    let body = match ByteStream::from_path(Path::new(local_path)).await {
        Ok(body) => body,
        Err(err) => {
            warn!("Could not read {} to cache it: {}", local_path, err);
            return;
        }
    };
    match s3_client
        .put_object()
        .bucket(&bucket_name)
        .key(&key)
        .body(body)
        .send()
        .await
    {
        Ok(_) => info!("Cached in the binaries bucket: {}", key),
        Err(err) => warn!("Could not cache {} in the binaries bucket: {}", key, err),
    }
}

/// Whether the binary for release `tag` is already on disk.
///
/// The installed binaries are the record of what has been downloaded, rather
/// than a "latest installed version" marker: such a marker only moves forward,
/// so a patch published after a higher version (Scarb backports one every few
/// releases, and a stable release can follow a release candidate of the next
/// minor) would sort below it and never install.
async fn is_installed(tool: Tool, binaries_dir: &str, version: &Version, tag: &str) -> bool {
    match tool {
        // The file name follows from the tag, so the binary is its own record.
        Tool::Sozo => Path::new(&tool.binary_path(binaries_dir, version)).exists(),
        // The file name carries the Cairo version, which the tag does not give
        // us, so the marker written at install time holds the name it got. The
        // binary is checked too, so removing one pulls it again.
        Tool::Scarb => {
            match tokio_fs::read_to_string(tool.installed_marker_path(binaries_dir, tag)).await {
                Ok(binary_name) => Path::new(&format!(
                    "{}/{}",
                    tool.binary_dir(binaries_dir),
                    binary_name.trim()
                ))
                .exists(),
                Err(_) => false,
            }
        }
    }
}

/// Record that release `tag` installed the binary named `binary_name`.
async fn record_installed(
    tool: Tool,
    binaries_dir: &str,
    tag: &str,
    binary_name: &str,
) -> Result<(), Box<dyn Error>> {
    let marker_path = tool.installed_marker_path(binaries_dir, tag);
    if let Some(marker_dir) = Path::new(&marker_path).parent() {
        tokio_fs::create_dir_all(marker_dir).await?;
    }
    tokio_fs::write(&marker_path, binary_name.as_bytes()).await?;
    Ok(())
}

// Downloads every supported Scarb/Sozo release that is not installed yet.
// The logic is:
// 1. Fetch every release from Github (see `get_all_releases`).
// 2. Drop the releases at or below `latest_unsupported_tag`, and the ones
//    already on disk (see `is_installed`). Release order does not matter here:
//    a patch published after a higher version installs like any other.
// 3. Download the tar.gz and extract it. Only for SCARB: run the binary to read
//    the Cairo version it ships and name it after that, e.g. `scarb --version`
//    reporting `cairo: 2.9.1` gives `scarb_cairo_v2.9.1`.
//    NOTE: the Scarb binary is named after the Cairo version, not the Scarb tag,
//          so scarb vX.X.X can land under scarb_cairo_vY.Y.Y.
// 4. Record the install so the release is recognised on the next run, and
//    remove the archive and the extracted folder.
pub async fn check_periodically_updates(
    repo: &str,
    tool: Tool,
    // We support here every version above that
    latest_unsupported_tag: &str,
    binary_path_in_extracted_folder: &str,
    s3_client: &S3Client,
) -> Result<(), Box<dyn Error>> {
    let binaries_dir_path_string =
        std::env::var("BINARIES_SAVE_DIRECTORY_PATH").unwrap_or_else(|_| ".".to_string());
    tokio_fs::create_dir_all(&binaries_dir_path_string).await?;

    // Releases at or below this one are too old to be worth installing. It is a
    // fixed support boundary, not a record of progress, so it never moves.
    let latest_unsupported_version = Version::parse(latest_unsupported_tag).unwrap();

    let all_releases = get_all_releases(repo).await?;

    let mut releases: Vec<(Version, Release)> = all_releases
        .into_iter()
        .filter_map(|release| {
            // Parse version once per release
            let version_str = parse_version_from_tag(&release.tag_name);
            let version = Version::parse(&version_str).ok()?;

            if version <= latest_unsupported_version {
                debug!(
                    "Skipping {} release {} (parsed {}): not above the oldest supported version {}.",
                    tool, release.tag_name, version, latest_unsupported_version
                );
                return None;
            }

            Some((version, release))
        })
        .collect();

    // Oldest first, so an interrupted run leaves off the newest releases rather
    // than a gap in the middle.
    releases.sort_by(|a, b| a.0.cmp(&b.0));

    for (release_version, release) in releases {
        if is_installed(
            tool,
            &binaries_dir_path_string,
            &release_version,
            &release.tag_name,
        )
        .await
        {
            debug!(
                "Skipping {} release {}: already installed.",
                tool, release.tag_name
            );
            continue;
        }

        let asset_suffix = asset_suffix_for_arch(tool)?;
        if let Some(asset) = release
            .assets
            .iter()
            .find(|asset| asset.name.ends_with(asset_suffix))
        {
            let output_path_string = format!("{}/{}", binaries_dir_path_string, asset.name);
            let tar_gz_output_path = Path::new(&output_path_string);
            debug!("Downloading {}", asset.browser_download_url);
            download_file(&asset.browser_download_url, tar_gz_output_path).await?;
            info!("Downloaded to: {:?}", tar_gz_output_path);

            if asset.name.ends_with(".tar.gz") {
                if tool == Tool::Scarb {
                    let binaries_dir_path = Path::new(&binaries_dir_path_string);
                    debug!("Extracting to: {:?}", binaries_dir_path);
                    extract_tar_gz(tar_gz_output_path, binaries_dir_path).await?;
                    let extracted_tar_gz_folder_path =
                        output_path_string.trim_end_matches(".tar.gz");
                    let extracted_binary_path = format!(
                        "{}{}",
                        extracted_tar_gz_folder_path, &binary_path_in_extracted_folder
                    );
                    let version = extract_cairo_version(extracted_binary_path.as_str()).await?;
                    let extracted_binary_destination_path =
                        tool.binary_path(&binaries_dir_path_string, &version);

                    // Move the binary to the destination directory (e.g. binaries/<tool>)
                    if let Some(destination_dir) =
                        Path::new(&extracted_binary_destination_path).parent()
                    {
                        tokio_fs::create_dir_all(destination_dir).await?;
                    }
                    tokio_fs::rename(&extracted_binary_path, &extracted_binary_destination_path)
                        .await?;
                    // Remove the extracted tar.gz folder
                    tokio::fs::remove_dir_all(extracted_tar_gz_folder_path).await?;
                    // Record the install under the release tag: the name above
                    // carries the Cairo version, which nothing can derive from
                    // the tag without downloading the binary again.
                    let binary_name = tool.binary_name(&version);
                    record_installed(
                        tool,
                        &binaries_dir_path_string,
                        &release.tag_name,
                        &binary_name,
                    )
                    .await?;
                    info!(
                        "Extracted successfully: {}",
                        &extracted_binary_destination_path
                    );

                    // The marker goes to the bucket with the binary: without it
                    // a restored binary cannot be matched back to this release,
                    // and the release would be downloaded again to find out
                    // which Cairo version it ships.
                    cache_in_bucket(
                        s3_client,
                        tool,
                        &binary_name,
                        &extracted_binary_destination_path,
                    )
                    .await;
                    cache_in_bucket(
                        s3_client,
                        tool,
                        &installed_marker_relative_path(&release.tag_name),
                        &tool.installed_marker_path(&binaries_dir_path_string, &release.tag_name),
                    )
                    .await;
                }
                if tool == Tool::Sozo {
                    let extract_path = format!(
                        "{}/{}",
                        &binaries_dir_path_string,
                        &asset.name.trim_end_matches(".tar.gz")
                    );
                    let extract_path_dir_path = Path::new(extract_path.as_str());
                    info!("Extracting to: {:?}", extract_path_dir_path);
                    extract_tar_gz(tar_gz_output_path, extract_path_dir_path).await?;
                    let extracted_binary_path =
                        format!("{}{}", &extract_path, &binary_path_in_extracted_folder);
                    let extracted_binary_destination_path =
                        tool.binary_path(&binaries_dir_path_string, &release_version);

                    // Move the binary to the destination directory (e.g. binaries/<tool>)
                    if let Some(destination_dir) =
                        Path::new(&extracted_binary_destination_path).parent()
                    {
                        tokio_fs::create_dir_all(destination_dir).await?;
                    }
                    tokio_fs::rename(&extracted_binary_path, &extracted_binary_destination_path)
                        .await?;
                    // Remove the extracted tar.gz folder
                    tokio::fs::remove_dir_all(&extract_path).await?;
                    info!(
                        "Extracted successfully: {}",
                        &extracted_binary_destination_path
                    );

                    // No marker to go with it: a Sozo binary is named after the
                    // tag, so restoring it is enough to recognise the release.
                    cache_in_bucket(
                        s3_client,
                        tool,
                        &tool.binary_name(&release_version),
                        &extracted_binary_destination_path,
                    )
                    .await;
                }
            }
            // Remove the tar.gz file
            tokio::fs::remove_file(tar_gz_output_path).await?;
        } else {
            info!("No compatible assets found in the release.");
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{is_installed, record_installed};
    use crate::binary_names::Tool;
    use semver::Version;
    use std::fs;
    use std::sync::atomic::{AtomicUsize, Ordering};

    /// A directory of its own per test, so the installs of one are not the
    /// installs of another.
    fn temp_binaries_dir() -> String {
        static COUNTER: AtomicUsize = AtomicUsize::new(0);
        let path = std::env::temp_dir().join(format!(
            "walnut-binaries-test-{}-{}",
            std::process::id(),
            COUNTER.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(&path).unwrap();
        path.to_str().unwrap().to_string()
    }

    fn touch(path: &str) {
        fs::create_dir_all(std::path::Path::new(path).parent().unwrap()).unwrap();
        fs::write(path, b"").unwrap();
    }

    #[tokio::test]
    async fn a_sozo_release_is_installed_when_its_binary_is_there() {
        // The file name follows from the tag, so nothing else has to be kept -
        // a binary restored from the bucket is recognised on its own.
        let dir = temp_binaries_dir();
        let version = Version::parse("1.8.1").unwrap();

        assert!(!is_installed(Tool::Sozo, &dir, &version, "sozo/v1.8.1").await);

        touch(&Tool::Sozo.binary_path(&dir, &version));
        assert!(is_installed(Tool::Sozo, &dir, &version, "sozo/v1.8.1").await);
    }

    #[tokio::test]
    async fn a_scarb_release_is_installed_only_once_it_has_been_recorded() {
        // A Scarb binary is named after the Cairo version it ships, which the
        // tag does not give us - the binary sitting there under some other name
        // cannot be matched to this release without the marker. This is why the
        // marker is cached in the bucket alongside the binary.
        let dir = temp_binaries_dir();
        let version = Version::parse("2.12.0").unwrap();
        let binary_name = Tool::Scarb.binary_name("2.11.4");
        touch(&format!("{}/{}", Tool::Scarb.binary_dir(&dir), binary_name));

        assert!(!is_installed(Tool::Scarb, &dir, &version, "v2.12.0").await);

        record_installed(Tool::Scarb, &dir, "v2.12.0", &binary_name)
            .await
            .unwrap();
        assert!(is_installed(Tool::Scarb, &dir, &version, "v2.12.0").await);
    }

    #[tokio::test]
    async fn a_scarb_release_whose_binary_was_removed_installs_again() {
        // The marker on its own is not proof: a binary deleted off the disk has
        // to come back.
        let dir = temp_binaries_dir();
        let version = Version::parse("2.12.0").unwrap();
        let binary_name = Tool::Scarb.binary_name("2.12.0");
        let binary_path = format!("{}/{}", Tool::Scarb.binary_dir(&dir), binary_name);

        touch(&binary_path);
        record_installed(Tool::Scarb, &dir, "v2.12.0", &binary_name)
            .await
            .unwrap();
        assert!(is_installed(Tool::Scarb, &dir, &version, "v2.12.0").await);

        fs::remove_file(&binary_path).unwrap();
        assert!(!is_installed(Tool::Scarb, &dir, &version, "v2.12.0").await);
    }

    #[tokio::test]
    async fn two_releases_shipping_one_cairo_version_share_a_binary() {
        // Several Scarb releases ship the same Cairo version and so land on one
        // file name. Each gets its own marker, so neither is downloaded twice.
        let dir = temp_binaries_dir();
        let cairo = Version::parse("2.12.0").unwrap();
        let binary_name = Tool::Scarb.binary_name(&cairo);
        touch(&format!("{}/{}", Tool::Scarb.binary_dir(&dir), binary_name));

        for tag in ["v2.12.0", "v2.12.1"] {
            record_installed(Tool::Scarb, &dir, tag, &binary_name)
                .await
                .unwrap();
        }

        assert!(
            is_installed(
                Tool::Scarb,
                &dir,
                &Version::parse("2.12.0").unwrap(),
                "v2.12.0"
            )
            .await
        );
        assert!(
            is_installed(
                Tool::Scarb,
                &dir,
                &Version::parse("2.12.1").unwrap(),
                "v2.12.1"
            )
            .await
        );
    }

    #[tokio::test]
    async fn a_release_published_after_a_higher_one_is_not_taken_for_installed() {
        // The case a "latest installed version" marker could not express: 2.16.1
        // was released after 2.17.0-rc.1, and used to need a hardcoded exception
        // to be installed at all.
        let dir = temp_binaries_dir();
        let rc = Version::parse("2.17.0-rc.1").unwrap();
        let patch = Version::parse("2.16.1").unwrap();
        assert!(patch < rc);

        record_installed(
            Tool::Scarb,
            &dir,
            "v2.17.0-rc.1",
            &Tool::Scarb.binary_name(&rc),
        )
        .await
        .unwrap();
        touch(&Tool::Scarb.binary_path(&dir, &rc));

        assert!(is_installed(Tool::Scarb, &dir, &rc, "v2.17.0-rc.1").await);
        assert!(!is_installed(Tool::Scarb, &dir, &patch, "v2.16.1").await);
    }
}
