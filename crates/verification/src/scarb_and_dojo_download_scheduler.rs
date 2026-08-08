use async_compression::tokio::bufread::GzipDecoder;
use async_tar::Archive;
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
use tracing::{debug, info};

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

async fn get_all_releases(repo: &str) -> Result<Vec<Release>, Box<dyn std::error::Error>> {
    let url = format!("https://api.github.com/repos/{}/releases", repo);
    let client = Client::new();
    let response = client
        .get(&url)
        .header("User-Agent", "rust-app")
        .send()
        .await?;
    let releases: Vec<Release> = response.json().await?;
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

fn get_sozo_file_name_for_arch() -> Result<String, Box<dyn Error>> {
    let architecture = ARCH;
    let file_name = match architecture {
        "x86_64" => "linux_amd64.tar.gz",
        "aarch64" => "darwin_arm64.tar.gz",
        _ => {
            return Err(Box::from(format!(
                "Unsupported architecture: {}",
                architecture
            )))
        }
    };
    Ok(file_name.to_string())
}

fn get_scarb_file_name_for_arch() -> Result<String, Box<dyn Error>> {
    let architecture = ARCH;
    let file_name = match architecture {
        "x86_64" => "x86_64-unknown-linux-gnu.tar.gz",
        "aarch64" => "aarch64-apple-darwin.tar.gz",
        _ => {
            return Err(Box::from(format!(
                "Unsupported architecture: {}",
                architecture
            )))
        }
    };
    Ok(file_name.to_string())
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
    versioning_file_name: &str,
) -> Result<(), Box<dyn Error>> {
    check_periodically_updates(
        repo,
        versioning_file_name,
        "sozo",
        "1.0.12",
        "/sozo",
        "/sozo/sozo_",
    )
    .await
}

pub async fn check_periodically_scarb_updates(
    repo: &str,
    versioning_file_name: &str,
) -> Result<(), Box<dyn std::error::Error>> {
    check_periodically_updates(
        repo,
        versioning_file_name,
        "scarb",
        "2.8.5",
        "/bin/scarb",
        "/scarb/scarb_cairo_",
    )
    .await
}

// The function does check in Github for new versions of Scarb/Sozo and downloads them if available
// The logic is:
// 1. Fetch releases JSON from Github. It contains all the versions of Scarb/Sozo in JSON format.
// 2. Filter from the releases version only below the latest downloaded (saved in *_LATEST_VERSION_FILE_NAME file)
//    and sort them: oldest first. Now process will iterate and download all missing binaries.
// 3. Download tar.gz file, extract. Only for SCARB: Then check the Cairo version of the binary and use this
//    version to name the binary e.g. scarb --version otuputs cairo: 2.9.1 then binary is names scarb_cairo_v2.9.1
//    NOTE: We save scarb binary with name of Cairo version, not Scarb tag name (e.g. scarb_cairo_v2.9.1)
//          it means that scarb_vX.X.X can be saved differently like scarb_cairo_vY.Y.Y because as
//          version we use Cairo version (not Scarb version)
// 4. Latest download tag version (also for SCARB it's tag, not Cairo version) is saved in *_LATEST_VERSION_FILE_NAME file.
//    Unnecessary files and folders are removed.
pub async fn check_periodically_updates(
    repo: &str,
    versioning_file_name: &str,
    tool_name: &str,
    // We support here every version above that
    latest_unsupported_tag: &str,
    binary_path_in_extracted_folder: &str,
    binary_destination_path_prefix: &str,
) -> Result<(), Box<dyn Error>> {
    let binaries_dir_path_string =
        std::env::var("BINARIES_SAVE_DIRECTORY_PATH").unwrap_or_else(|_| ".".to_string());
    tokio_fs::create_dir_all(&binaries_dir_path_string).await?;
    let mut latest_installed_tag = Version::parse(latest_unsupported_tag).unwrap();

    // Check if the version file exists
    if !Path::new(&versioning_file_name).exists() {
        info!(
            "{} last tag file does not exist. Creating it...",
            &tool_name
        );
        File::create(&versioning_file_name)
            .await?
            .write_all(latest_unsupported_tag.as_bytes())
            .await?;
    } else {
        let content = tokio_fs::read_to_string(&versioning_file_name).await?;
        let content_parsed = parse_version_from_tag(content.trim());
        let latest_installed_tag_from_file = match Version::parse(content_parsed.as_str()) {
            Ok(ver) => ver,
            Err(_) => {
                return Err(Box::from(format!(
                    "Invalid (corrupted) latest {} tag value found in file: {}",
                    &tool_name, &versioning_file_name
                )));
            }
        };
        latest_installed_tag = latest_installed_tag_from_file.clone();
    }

    let all_releases = get_all_releases(repo).await?;

    // NOTE: Pre-check if 2.9.4 is already installed
    // The 2.9.4 is release after 2.10.0
    let version_2_9_4 = Version::parse("2.9.4").unwrap();
    let expected_2_9_4_path = format!(
        "{}{}v2.9.4",
        binaries_dir_path_string, binary_destination_path_prefix
    );
    let should_include_2_9_4 = !Path::new(&expected_2_9_4_path).exists();

    // NOTE: Pre-check if 2.16.1 is already installed
    // The 2.16.1 is release after 2.17.0-rc.0 and 2.17.0-rc.1
    let version_2_16_1 = Version::parse("2.16.1").unwrap();
    let expected_2_16_1_path = format!(
        "{}{}v2.16.1",
        binaries_dir_path_string, binary_destination_path_prefix
    );
    let should_include_2_16_1 = !Path::new(&expected_2_16_1_path).exists();

    // Process all releases in a single pass
    let mut releases: Vec<(Version, Release)> = all_releases
        .into_iter()
        .filter_map(|release| {
            // Parse version once per release
            let version_str = parse_version_from_tag(&release.tag_name);
            let version = Version::parse(&version_str).ok()?;

            // Check for new version and for 2.9.4 and 2.16.1
            let is_newer = version > latest_installed_tag;
            let is_2_9_4 = version == version_2_9_4 && should_include_2_9_4;
            let is_2_16_1 = version == version_2_16_1 && should_include_2_16_1;

            if !is_newer {
                debug!(
                    "Skipping {} release {} (parsed {}): not newer than current latest installed tag {}.",
                    tool_name, release.tag_name, version, latest_installed_tag
                );
            }

            (is_newer || is_2_9_4 || is_2_16_1).then_some((version, release))
        })
        .collect();

    // Sort once
    releases.sort_by(|a, b| a.0.cmp(&b.0));

    for (_, release) in releases {
        let file_name = match tool_name {
            "scarb" => get_scarb_file_name_for_arch()?,
            "sozo" => get_sozo_file_name_for_arch()?,
            _ => panic!("Unknown tool name: {}", tool_name),
        };
        if let Some(asset) = release
            .assets
            .iter()
            .find(|asset| asset.name.ends_with(file_name.as_str()))
        {
            let output_path_string = format!("{}/{}", binaries_dir_path_string, asset.name);
            let tar_gz_output_path = Path::new(&output_path_string);
            debug!("Downloading {}", asset.browser_download_url);
            download_file(&asset.browser_download_url, tar_gz_output_path).await?;
            info!("Downloaded to: {:?}", tar_gz_output_path);

            if asset.name.ends_with(".tar.gz") {
                if tool_name == "scarb" {
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
                    let tag_name = format!("v{}", version);
                    let extracted_binary_destination_path = format!(
                        "{}{}{}",
                        binaries_dir_path_string, &binary_destination_path_prefix, tag_name
                    );

                    // Move the binary to the destination directory (e.g. binaries/<tool_name>)
                    if let Some(destination_dir) =
                        Path::new(&extracted_binary_destination_path).parent()
                    {
                        tokio_fs::create_dir_all(destination_dir).await?;
                    }
                    tokio_fs::rename(&extracted_binary_path, &extracted_binary_destination_path)
                        .await?;
                    // Remove the extracted tar.gz folder
                    tokio::fs::remove_dir_all(extracted_tar_gz_folder_path).await?;
                    // Save the latest downloaded tool version to the file
                    // NOTE: We are not update the latest_installed_tag_from_file in case it is
                    // 2.9.4 or 2.16.1
                    if version != version_2_9_4 && version != version_2_16_1 {
                        tokio_fs::write(&versioning_file_name, release.tag_name.as_bytes()).await?;
                    }
                    info!(
                        "Extracted successfully: {}",
                        &extracted_binary_destination_path
                    );
                }
                if tool_name == "sozo" {
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
                    let version_str = parse_version_from_tag(&release.tag_name);
                    let version = Version::parse(&version_str)?;
                    let tag_name = format!("v{}", version);
                    let extracted_binary_destination_path = format!(
                        "{}{}{}",
                        binaries_dir_path_string, binary_destination_path_prefix, tag_name
                    );

                    // Move the binary to the destination directory (e.g. binaries/<tool_name>)
                    if let Some(destination_dir) =
                        Path::new(&extracted_binary_destination_path).parent()
                    {
                        tokio_fs::create_dir_all(destination_dir).await?;
                    }
                    tokio_fs::rename(&extracted_binary_path, &extracted_binary_destination_path)
                        .await?;
                    // Remove the extracted tar.gz folder
                    tokio::fs::remove_dir_all(&extract_path).await?;
                    // Save the latest downloaded tool version to the file
                    // NOTE! We save here the tag, not cairo version (relevant for Scarb only)
                    tokio_fs::write(&versioning_file_name, release.tag_name.as_bytes()).await?;
                    info!(
                        "Extracted successfully: {}",
                        &extracted_binary_destination_path
                    );
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
