use anyhow::{anyhow, Context, Result};
use async_compression::tokio::bufread::GzipDecoder;
use async_tar::Archive;
use aws_sdk_s3::primitives::ByteStream;
use futures::StreamExt;
use lazy_regex::regex;
use reqwest::Client;
use semver::Version;
use serde::Deserialize;
use serde_json::{json, Value};
use std::collections::BTreeMap;
use std::env::consts::ARCH;
use std::path::{Path, PathBuf};
use time::format_description::well_known::Rfc3339;
use time::OffsetDateTime;
use tokio::fs as tokio_fs;
use tokio::fs::File;
use tokio::io::{AsyncWriteExt, BufReader};
use tokio::process::Command;
use tokio_util::compat::TokioAsyncReadCompatExt;
use tracing::{debug, error, info, warn};

use crate::scarb::{is_cairo_version_supported, is_dojo_version_supported};

/// The metadata object the binaries bucket carries beside each tool's binaries,
/// mirrored to `<BINARIES_SAVE_DIRECTORY_PATH>/<tool>/versions.json` at startup.
/// Written by backfill-binaries-bucket.sh and, when a check installs something
/// new, by this module.
const SIDECAR_NAME: &str = "versions.json";

/// `/releases` is paginated at 100; this only stops a bug from paging forever.
/// Scarb is at 129 releases, so it is roughly sixteen times the present need.
const MAX_RELEASE_PAGES: u32 = 20;

/// Below this, a Scarb release cannot be shipping a Cairo the verifier accepts,
/// so it is not worth fetching to find out what it does ship.
///
/// This is the one place a Cairo version is inferred rather than read off the
/// binary, and it leans on Cairo never being ahead of the Scarb release carrying
/// it — v2.6.5 ships 2.6.4, v2.20.1 ships 2.20.0, and so on down the line.
/// Nothing upstream promises that, which is why it is only ever used to rule out
/// releases that are already frozen history. Everything at or above it is
/// downloaded and asked.
///
/// It is worth roughly thirty releases a machine would otherwise fetch, unpack
/// and run once, purely to learn they are too old to use.
const OLDEST_INTERESTING_SCARB_TAG: (u64, u64, u64) = (2, 6, 3);

// Struct to deserialize GitHub API release response
#[derive(Debug, Deserialize)]
struct Release {
    tag_name: String,
    assets: Vec<Asset>,
    /// Release candidates install under the name of the stable release they
    /// precede — the Cairo version drops its `-rc.N` on the way to a filename —
    /// so they are dropped rather than left to impersonate it.
    #[serde(default)]
    prerelease: bool,
}

#[derive(Debug, Deserialize)]
struct Asset {
    browser_download_url: String,
    name: String,
}

/// The two toolchains this module keeps up to date. Everything that differs
/// between them lives here rather than in the flow below, which is identical
/// for both.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tool {
    Scarb,
    Sozo,
}

impl Tool {
    pub fn as_str(self) -> &'static str {
        match self {
            Tool::Scarb => "scarb",
            Tool::Sozo => "sozo",
        }
    }

    /// The name of the field the sidecar records a version under.
    fn version_field(self) -> &'static str {
        match self {
            Tool::Scarb => "cairo",
            Tool::Sozo => "dojo",
        }
    }

    /// The release asset built for this machine, matched by suffix the way the
    /// bucket backfill matches it — one rule covers both of Dojo's naming eras,
    /// `dojo_v1.6.0_*` and `sozo_v1.8.1_*`.
    fn asset_suffix(self) -> Result<&'static str> {
        Ok(match (self, ARCH) {
            (Tool::Scarb, "x86_64") => "x86_64-unknown-linux-gnu.tar.gz",
            (Tool::Scarb, "aarch64") => "aarch64-apple-darwin.tar.gz",
            (Tool::Sozo, "x86_64") => "linux_amd64.tar.gz",
            (Tool::Sozo, "aarch64") => "darwin_arm64.tar.gz",
            (_, other) => return Err(anyhow!("Unsupported architecture: {}", other)),
        })
    }

    /// The filename the verifier builds when it wants this version, and so the
    /// only name an installed binary may have. One spelling per tool, the
    /// version written the way it is everywhere else — the pre-2.8 Scarb
    /// binaries used to be `scarb_cairo_v_2_6_3` instead, which meant the
    /// separator was a third thing to agree on between here and the two
    /// lookups in crates/verification/src/scarb.rs.
    pub fn object_name(self, version: &Version) -> String {
        format!(
            "{}_v{}.{}.{}",
            match self {
                Tool::Scarb => "scarb_cairo",
                Tool::Sozo => "sozo",
            },
            version.major,
            version.minor,
            version.patch
        )
    }

    /// Where a version of this tool lives on disk. The one place a path to a
    /// toolchain is built, by the check that installs it and by the verifier
    /// that runs it - the two agreeing is the whole point.
    pub fn binary_path(self, binaries_dir: &str, version: &Version) -> String {
        format!(
            "{}/{}/{}",
            binaries_dir,
            self.as_str(),
            self.object_name(version)
        )
    }

    /// Whether a release is old enough that it is not worth fetching to find
    /// out what it ships. See `OLDEST_INTERESTING_SCARB_TAG` for why this is
    /// sound only as a lower bound, and only for Scarb — a Sozo release is named
    /// after the version it installs, so `is_supported` answers directly.
    fn is_too_old_to_consider(self, tag_version: &Version) -> bool {
        match self {
            Tool::Scarb => {
                *tag_version
                    < Version::new(
                        OLDEST_INTERESTING_SCARB_TAG.0,
                        OLDEST_INTERESTING_SCARB_TAG.1,
                        OLDEST_INTERESTING_SCARB_TAG.2,
                    )
            }
            Tool::Sozo => false,
        }
    }

    /// Whether the verifier would accept a build of this version. This is what
    /// replaced a hardcoded floor: the scheduler now looks at every release
    /// upstream has, and the reason to skip one is that nothing could ask for
    /// it, not that it happens to be older than a number someone wrote down.
    fn is_supported(self, version: &Version) -> bool {
        match self {
            Tool::Scarb => is_cairo_version_supported((
                version.major as u32,
                version.minor as u32,
                version.patch as u32,
            )),
            Tool::Sozo => is_dojo_version_supported(&format!(
                "{}.{}.{}",
                version.major, version.minor, version.patch
            )),
        }
    }
}

// --- GitHub ----------------------------------------------------------------

/// A GitHub API request, authenticated when a token is around.
///
/// Anonymous callers get 60 requests an hour per IP. A check costs one request
/// per page — three or four for both tools — so anonymous works, but it leaves
/// no room for anything else on the box and none for a retry. `GITHUB_TOKEN`
/// raises that to 5000.
fn github_request(client: &Client, url: &str) -> reqwest::RequestBuilder {
    let request = client.get(url).header("User-Agent", "walnut-server");
    match std::env::var("GITHUB_TOKEN") {
        Ok(token) if !token.is_empty() => request.bearer_auth(token),
        _ => request,
    }
}

/// Every release a repository has, oldest page last.
///
/// The pagination is the point. `/releases` without `per_page` answers with the
/// newest 30 and no indication that there are more, which is how a version that
/// scrolled off that page became unreachable: it could not be found upstream and
/// it was not on the disk either. Scarb is at 129 releases, so the unpaginated
/// form now sees under a quarter of them.
async fn list_all_releases(repo: &str) -> Result<Vec<Release>> {
    let client = Client::new();
    let mut releases: Vec<Release> = Vec::new();

    for page in 1..=MAX_RELEASE_PAGES {
        let url = format!(
            "https://api.github.com/repos/{}/releases?per_page=100&page={}",
            repo, page
        );
        let response = github_request(&client, &url)
            .send()
            .await
            .with_context(|| format!("requesting {}", url))?
            // Without this a rate-limited 403 arrives as a JSON object and
            // fails to deserialize into a list, which says nothing about why.
            .error_for_status()
            .with_context(|| format!("requesting {}", url))?;

        let page_of_releases: Vec<Release> = response
            .json()
            .await
            .with_context(|| format!("decoding {}", url))?;

        let is_last_page = page_of_releases.len() < 100;
        releases.extend(page_of_releases);
        if is_last_page {
            break;
        }
    }

    Ok(releases)
}

// Parse version from tag name, handling both "v1.8.0" and "sozo/v1.8.1" formats
pub fn parse_version_from_tag(tag_name: &str) -> String {
    let mut version_str = tag_name.trim();

    if version_str.starts_with("sozo/") {
        version_str = &version_str[5..];
    }
    version_str = version_str.trim_start_matches('v');
    version_str.to_string()
}

// --- which Cairo version does a Scarb release ship? ------------------------

// --- the sidecar -----------------------------------------------------------

/// The bucket's metadata for one tool, and whatever this run has to add to it.
///
/// Two jobs. It is the cache that lets a check decide, without downloading
/// anything, which releases it already has: `releases` maps a release tag to the
/// version it installs as, which for Scarb is the only way to connect the two.
/// And it is the record the bucket carries about its own contents, which this
/// module now writes as well as reads.
struct Sidecar {
    /// The document as read, so fields this build knows nothing about survive
    /// being written back.
    document: Value,
    /// Release tag -> the version it installs as.
    known: BTreeMap<String, Version>,
    /// The subset of `known` this run worked out, and the objects it uploaded.
    /// Only these are merged into the copy in the bucket, so a concurrent writer
    /// loses nothing but a race on the same key.
    added_releases: BTreeMap<String, Version>,
    added_objects: Vec<(String, Value)>,
}

impl Sidecar {
    fn local_path(binaries_dir: &str, tool: Tool) -> String {
        format!("{}/{}/{}", binaries_dir, tool.as_str(), SIDECAR_NAME)
    }

    /// Read the mirrored copy. Every way of not having one — no file, not JSON,
    /// a `releases` map full of things that are not versions — is an empty
    /// cache, which costs a slower first check and nothing else.
    async fn load(binaries_dir: &str, tool: Tool) -> Self {
        let path = Self::local_path(binaries_dir, tool);
        let document = match tokio_fs::read_to_string(&path).await {
            Ok(contents) => serde_json::from_str::<Value>(&contents).unwrap_or_else(|err| {
                warn!("{} is not readable as JSON ({}); ignoring it", path, err);
                json!({})
            }),
            Err(err) => {
                debug!("no sidecar at {} ({})", path, err);
                json!({})
            }
        };

        let known = known_releases(&document);

        Sidecar {
            document,
            known,
            added_releases: BTreeMap::new(),
            added_objects: Vec::new(),
        }
    }

    fn record_release(&mut self, tag: &str, version: Version) {
        if self.known.get(tag) == Some(&version) {
            return;
        }
        self.known.insert(tag.to_string(), version.clone());
        self.added_releases.insert(tag.to_string(), version);
    }

    fn record_object(
        &mut self,
        tool: Tool,
        name: &str,
        version: &Version,
        release: &Release,
        asset: &Asset,
    ) {
        self.added_objects.push((
            name.to_string(),
            json!({
                tool.version_field(): format!("{}.{}.{}", version.major, version.minor, version.patch),
                "release": release.tag_name,
                "asset": asset.browser_download_url,
            }),
        ));
    }

    fn has_updates(&self) -> bool {
        !self.added_releases.is_empty() || !self.added_objects.is_empty()
    }

    /// Merge this run's additions into a sidecar document — the local mirror, or
    /// the one just read back out of the bucket.
    fn apply_to(&self, document: &mut Value, tool: Tool, arch_folder: Option<&str>) {
        if !document.is_object() {
            *document = json!({});
        }
        let object = document.as_object_mut().expect("just made it an object");

        object.insert("tool".into(), json!(tool.as_str()));
        // Only when it is known: the bucket spells this differently from Rust
        // ("arm64" for "aarch64"), and the mapping belongs to the caller that
        // publishes. Guessing it into a local file would plant a wrong answer.
        if let Some(arch_folder) = arch_folder {
            object.insert("arch".into(), json!(arch_folder));
        }
        object.insert("written_by".into(), json!("starknet-debugger-server"));
        if let Ok(now) = OffsetDateTime::now_utc().format(&Rfc3339) {
            object.insert("written_at".into(), json!(now));
        }

        // Each map is created only when there is something to put in it. The
        // distinction matters for `binaries`: this module records the objects it
        // uploaded and nothing else — it never lists the bucket, which is the
        // backfill's job — so writing an empty map here would state that the
        // bucket holds no binaries, when what is true is that this run did not
        // find out. Absent says the second thing.
        if !self.added_releases.is_empty() {
            if let Some(releases) = object
                .entry("releases")
                .or_insert_with(|| json!({}))
                .as_object_mut()
            {
                for (tag, version) in &self.added_releases {
                    releases.insert(
                        tag.clone(),
                        json!(format!(
                            "{}.{}.{}",
                            version.major, version.minor, version.patch
                        )),
                    );
                }
            }
        }

        if !self.added_objects.is_empty() {
            if let Some(binaries) = object
                .entry("binaries")
                .or_insert_with(|| json!({}))
                .as_object_mut()
            {
                for (name, entry) in &self.added_objects {
                    binaries.insert(name.clone(), entry.clone());
                }
            }
        }

        // Informational: how current the bucket is, at a glance. Nothing reads
        // it back — `releases` is what a check consults — so it is allowed to
        // lag behind when a run installs nothing.
        let newest = self
            .added_objects
            .iter()
            .filter_map(|(_, entry)| entry.get("release")?.as_str())
            .filter_map(|tag| Some((Version::parse(&parse_version_from_tag(tag)).ok()?, tag)))
            .max_by(|left, right| left.0.cmp(&right.0));
        if let Some((newest_version, newest_tag)) = newest {
            let is_newer = object
                .get("newest_release")
                .and_then(Value::as_str)
                .and_then(|current| Version::parse(&parse_version_from_tag(current)).ok())
                .is_none_or(|current| newest_version > current);
            if is_newer {
                object.insert("newest_release".into(), json!(newest_tag));
            }
        }
    }

    async fn save_local(
        &self,
        binaries_dir: &str,
        tool: Tool,
        arch_folder: Option<&str>,
    ) -> Result<()> {
        let mut document = self.document.clone();
        self.apply_to(&mut document, tool, arch_folder);
        // Written beside the destination and renamed into place, the way the
        // startup download writes a binary: a process killed mid-write would
        // otherwise leave a truncated file, and while the next boot replaces it
        // from the bucket, a check between now and then would read it as an
        // empty cache and re-resolve everything.
        let path = Self::local_path(binaries_dir, tool);
        let partial_path = format!("{}.partial", path);
        tokio_fs::write(&partial_path, serde_json::to_vec_pretty(&document)?)
            .await
            .with_context(|| format!("writing {}", partial_path))?;
        tokio_fs::rename(&partial_path, &path)
            .await
            .with_context(|| format!("installing {}", path))?;
        Ok(())
    }
}

/// The release -> version mapping a sidecar document records, skipping anything
/// that does not parse. A malformed entry costs one release a slower check, and
/// must never cost the whole map.
fn known_releases(document: &Value) -> BTreeMap<String, Version> {
    document
        .get("releases")
        .and_then(Value::as_object)
        .map(|releases| {
            releases
                .iter()
                .filter_map(|(tag, version)| {
                    Some((tag.clone(), Version::parse(version.as_str()?).ok()?))
                })
                .collect()
        })
        .unwrap_or_default()
}

// --- writing back to the bucket --------------------------------------------

/// Where a check publishes what it installed.
///
/// The binaries bucket is the durable copy of every toolchain the verifier can
/// ask for, and until now only backfill-binaries-bucket.sh put anything in it —
/// so a release that landed between two runs of that script lived on one
/// machine's disk and nowhere else. A check that installs something now puts it
/// where the next machine will find it.
#[derive(Clone)]
pub struct BucketPublisher {
    client: aws_sdk_s3::Client,
    bucket: String,
    /// The bucket's spelling of this machine's architecture, which is not
    /// Rust's: the bucket says "arm64" where `ARCH` says "aarch64". Passed in by
    /// the caller that already owns that mapping rather than restated here.
    arch_folder: String,
}

impl BucketPublisher {
    pub fn new(client: aws_sdk_s3::Client, bucket: String, arch_folder: String) -> Self {
        BucketPublisher {
            client,
            bucket,
            arch_folder,
        }
    }

    pub fn arch_folder(&self) -> &str {
        &self.arch_folder
    }

    fn key_for(&self, tool: Tool, name: &str) -> String {
        format!("{}/{}/{}", tool.as_str(), self.arch_folder, name)
    }

    async fn put(&self, key: &str, body: ByteStream, content_type: &str) -> Result<()> {
        self.client
            .put_object()
            .bucket(&self.bucket)
            .key(key)
            .content_type(content_type)
            .body(body)
            .send()
            .await
            .with_context(|| format!("uploading {} to {}", key, self.bucket))?;
        Ok(())
    }

    /// `Ok(None)` only when the object is genuinely not there. Anything else —
    /// no permission, no network, a bad endpoint — is an error, because the one
    /// caller merges into what it reads and would otherwise take a failed read
    /// for an empty bucket and replace the file with this run's few entries.
    async fn get(&self, key: &str) -> Result<Option<Vec<u8>>> {
        let response = match self
            .client
            .get_object()
            .bucket(&self.bucket)
            .key(key)
            .send()
            .await
        {
            Ok(response) => response,
            Err(err) => {
                if err
                    .as_service_error()
                    .map(|err| err.is_no_such_key())
                    .unwrap_or(false)
                {
                    return Ok(None);
                }
                return Err(err).with_context(|| format!("reading {} from {}", key, self.bucket));
            }
        };
        Ok(Some(response.body.collect().await?.to_vec()))
    }

    /// Whether the bucket already has an object under this key.
    ///
    /// Anything other than a plain absence is an error rather than a `false`,
    /// for the same reason `get` treats it that way: a failed read taken for an
    /// empty slot is what turns a permissions problem into an overwrite.
    async fn has(&self, key: &str) -> Result<bool> {
        match self
            .client
            .head_object()
            .bucket(&self.bucket)
            .key(key)
            .send()
            .await
        {
            Ok(_) => Ok(true),
            Err(err) => {
                if err
                    .as_service_error()
                    .map(|err| err.is_not_found())
                    .unwrap_or(false)
                {
                    return Ok(false);
                }
                Err(err).with_context(|| format!("checking {} in {}", key, self.bucket))
            }
        }
    }

    /// Put a binary in the bucket, unless something is already there.
    ///
    /// A binary object is immutable: the key names the version it installs, so
    /// whatever is already under that key is a build of the same thing and
    /// there is nothing to gain by replacing it. What there is to lose is a
    /// build that was put there deliberately - one placed by hand does not
    /// exist on this machine's disk, so the check that skips an installed
    /// version never fires, and an unconditional upload would replace it on the
    /// first pass after a restart.
    ///
    /// The check is not atomic. Two machines installing the same version at
    /// once can both find the key empty and both upload, which is harmless -
    /// they are uploading the same release asset. It is a hand-placed object
    /// this protects, and that is not racing anything.
    async fn publish_binary(&self, tool: Tool, name: &str, path: &Path) -> Result<()> {
        let key = self.key_for(tool, name);
        if self.has(&key).await? {
            info!("{} is already in {}, leaving it alone", key, self.bucket);
            return Ok(());
        }
        let body = ByteStream::from_path(path)
            .await
            .with_context(|| format!("reading {} to upload", path.display()))?;
        self.put(&key, body, "application/octet-stream").await?;
        info!("Uploaded {} to {}", key, self.bucket);
        Ok(())
    }

    /// Merge this run's additions into the sidecar in the bucket.
    ///
    /// Read-modify-write, so two writers finishing at once can lose one set of
    /// additions. It is read back immediately before the write to keep that
    /// window at seconds, only a run that installed something writes at all, and
    /// the next backfill rebuilds the whole file from the bucket listing — so
    /// the failure mode is a missing entry until then, not a corrupted file.
    async fn publish_sidecar(&self, tool: Tool, sidecar: &Sidecar) -> Result<()> {
        let key = self.key_for(tool, SIDECAR_NAME);
        let mut document = match self.get(&key).await? {
            Some(bytes) => serde_json::from_slice(&bytes).unwrap_or_else(|err| {
                warn!("{} is not readable as JSON ({}); rewriting it", key, err);
                json!({})
            }),
            None => json!({}),
        };
        sidecar.apply_to(&mut document, tool, Some(&self.arch_folder));
        self.put(
            &key,
            ByteStream::from(serde_json::to_vec_pretty(&document)?),
            "application/json",
        )
        .await?;
        info!(
            "Recorded {} update(s) in {}",
            sidecar.added_objects.len(),
            key
        );
        Ok(())
    }
}

// --- installing ------------------------------------------------------------

async fn download_file(url: &str, output_path: &Path) -> Result<()> {
    let response = Client::new()
        .get(url)
        .header("User-Agent", "walnut-server")
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

async fn extract_tar_gz(archive_path: &Path, output_dir: &Path) -> Result<()> {
    let file = File::open(archive_path).await?;
    let buf_reader = BufReader::new(file);

    let gzip_decoder = GzipDecoder::new(buf_reader);
    let compat_reader = gzip_decoder.compat();
    let archive = Archive::new(compat_reader);

    archive.unpack(output_dir).await?;
    Ok(())
}

/// The tool's binary somewhere under an unpacked release.
///
/// Located rather than assumed: Scarb nests it under `<release>/bin/` and Dojo
/// ships it at the archive root, and both have moved things before.
fn find_binary(root: &Path, name: &str) -> Option<PathBuf> {
    let entries = std::fs::read_dir(root).ok()?;
    let mut directories = Vec::new();
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            directories.push(path);
        } else if path.file_name().and_then(|f| f.to_str()) == Some(name) {
            return Some(path);
        }
    }
    directories
        .into_iter()
        .find_map(|directory| find_binary(&directory, name))
}

// call scarb and extract the cairo_version
async fn extract_cairo_version(binary_path: &Path) -> Result<Version> {
    let output = Command::new(binary_path).arg("--version").output().await?;

    let scarb_output = String::from_utf8(output.stdout)?;

    let regex = regex!(r"cairo: ([\d\.]+)");
    let version_str = regex
        .captures(&scarb_output)
        .and_then(|caps| caps.get(1))
        .ok_or_else(|| anyhow!("Failed to find cairo version in scarb output"))?
        .as_str();

    Ok(Version::parse(version_str)?)
}

/// Fetch a release and unpack it, leaving the binary in a staging directory.
/// The caller decides what it is called and where it belongs.
async fn stage_release(tool: Tool, asset: &Asset, staging_dir: &Path) -> Result<PathBuf> {
    tokio_fs::create_dir_all(staging_dir).await?;
    let archive_path = staging_dir.join(&asset.name);

    debug!("Downloading {}", asset.browser_download_url);
    download_file(&asset.browser_download_url, &archive_path).await?;
    extract_tar_gz(&archive_path, staging_dir).await?;
    tokio_fs::remove_file(&archive_path).await.ok();

    let binary = find_binary(staging_dir, tool.as_str())
        .ok_or_else(|| anyhow!("no {} binary inside {}", tool.as_str(), asset.name))?;
    let mut permissions = tokio_fs::metadata(&binary).await?.permissions();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        permissions.set_mode(0o755);
    }
    #[cfg(not(unix))]
    {
        permissions.set_readonly(false);
    }
    tokio_fs::set_permissions(&binary, permissions).await?;

    Ok(binary)
}

// --- the check itself ------------------------------------------------------

pub async fn check_periodically_scarb_updates(
    repo: &str,
    publisher: Option<&BucketPublisher>,
) -> Result<()> {
    check_for_updates(repo, Tool::Scarb, publisher).await
}

pub async fn check_periodically_sozo_updates(
    repo: &str,
    publisher: Option<&BucketPublisher>,
) -> Result<()> {
    check_for_updates(repo, Tool::Sozo, publisher).await
}

/// Bring this machine's toolchains level with what upstream has released.
///
/// The shape is: list everything, work out what each release would install as,
/// skip the ones already on disk, fetch the rest, and put anything fetched into
/// the bucket so the next machine does not have to.
///
/// What it no longer does is track a high-water mark. That cursor existed for
/// one reason — a Scarb release does not say which Cairo version it ships, so
/// "do we have this one?" was unanswerable and "is it newer than last time?" was
/// the stand-in — and everything awkward followed from it: an arbitrary floor to
/// start from, two versions special-cased because they were published out of
/// order, a file on the data disk whose loss meant re-downloading everything,
/// and no way to notice a binary had gone missing. Resolving the release to its
/// Cairo version up front replaces the lot with a question about the filesystem.
pub async fn check_for_updates(
    repo: &str,
    tool: Tool,
    publisher: Option<&BucketPublisher>,
) -> Result<()> {
    let binaries_dir =
        std::env::var("BINARIES_SAVE_DIRECTORY_PATH").unwrap_or_else(|_| ".".to_string());
    let tool_dir = format!("{}/{}", binaries_dir, tool.as_str());
    tokio_fs::create_dir_all(&tool_dir).await?;

    let arch_folder = publisher.map(|publisher| publisher.arch_folder().to_string());
    let mut sidecar = Sidecar::load(&binaries_dir, tool).await;

    let releases = list_all_releases(repo).await?;
    info!("{}: {} release(s) upstream", tool.as_str(), releases.len());

    let candidates = installable_candidates(releases);

    let asset_suffix = tool.asset_suffix()?;
    let mut installed = 0usize;
    let mut already_present = 0usize;
    let mut unsupported = 0usize;
    let mut examined = 0usize;

    for (tag_version, release) in &candidates {
        if tool.is_too_old_to_consider(tag_version) {
            unsupported += 1;
            continue;
        }

        // What this release installs as. A Sozo release says so in its tag; a
        // Scarb release does not, and the only thing that knows is the binary
        // inside it. `known` is the record of every Scarb release some run has
        // already asked, and it is the whole reason this is not a download.
        let known_version = match tool {
            Tool::Sozo => Some(tag_version.clone()),
            Tool::Scarb => sidecar.known.get(&release.tag_name).cloned(),
        };

        match &known_version {
            // Seen before: decided here, without fetching anything.
            Some(known_version) => {
                if !tool.is_supported(known_version) {
                    unsupported += 1;
                    continue;
                }
                if Path::new(&tool.binary_path(&binaries_dir, known_version)).exists() {
                    already_present += 1;
                    continue;
                }
            }
            // Never seen: nothing on this machine can say what it installs as,
            // so it is fetched and asked. Whatever it turns out to be is written
            // down, so this happens once per release rather than once an hour.
            None => examined += 1,
        }

        let Some(asset) = release
            .assets
            .iter()
            .find(|asset| asset.name.ends_with(asset_suffix))
        else {
            info!(
                "{} {} has no asset for this machine",
                tool.as_str(),
                release.tag_name
            );
            continue;
        };

        match install(
            tool,
            release,
            asset,
            known_version.as_ref(),
            &binaries_dir,
            &tool_dir,
            publisher,
            &mut sidecar,
        )
        .await
        {
            Ok(true) => installed += 1,
            Ok(false) => already_present += 1,
            Err(err) => error!(
                "{}: installing {} failed: {:?}",
                tool.as_str(),
                release.tag_name,
                err
            ),
        }
    }

    info!(
        "{}: {} installed, {} already on disk, {} the verifier would not accept, {} fetched to find out what they were",
        tool.as_str(),
        installed,
        already_present,
        unsupported,
        examined
    );

    if sidecar.has_updates() {
        if let Err(err) = sidecar
            .save_local(&binaries_dir, tool, arch_folder.as_deref())
            .await
        {
            warn!(
                "{}: could not write the local sidecar: {:?}",
                tool.as_str(),
                err
            );
        }
        if let Some(publisher) = publisher {
            if let Err(err) = publisher.publish_sidecar(tool, &sidecar).await {
                // Not fatal: the binaries are installed and serving. What is
                // lost is the record, which the next backfill rebuilds.
                warn!(
                    "{}: could not record this run in the bucket: {:?}",
                    tool.as_str(),
                    err
                );
            }
        }
    }

    Ok(())
}

/// The releases worth considering, newest first.
///
/// The order matters: when two releases ship the same version, the first one
/// seen is installed and the rest are skipped as already present, so processing
/// newest-first is what makes the highest release win. That is the rule the
/// bucket backfill follows, and going the other way would quietly invert it.
///
/// Prereleases are dropped in both spellings — the flag upstream sets, and a
/// `-rc.N` in the tag — because an rc installs under the name of the stable
/// release it precedes: the Cairo version loses its prerelease part on the way
/// to a filename, and the rc would then stand in for a release that has not
/// happened yet.
fn installable_candidates(releases: Vec<Release>) -> Vec<(Version, Release)> {
    let mut candidates: Vec<(Version, Release)> = releases
        .into_iter()
        .filter(|release| !release.prerelease)
        .filter_map(|release| {
            let version = Version::parse(&parse_version_from_tag(&release.tag_name)).ok()?;
            version.pre.is_empty().then_some((version, release))
        })
        .collect();
    candidates.sort_by(|left, right| right.0.cmp(&left.0));
    candidates
}

/// Fetch one release, check what it really is, install it, and publish it.
/// Returns whether anything was installed.
#[allow(clippy::too_many_arguments)]
async fn install(
    tool: Tool,
    release: &Release,
    asset: &Asset,
    known_version: Option<&Version>,
    binaries_dir: &str,
    tool_dir: &str,
    publisher: Option<&BucketPublisher>,
    sidecar: &mut Sidecar,
) -> Result<bool> {
    let staging_dir = Path::new(binaries_dir).join(format!(
        ".staging-{}-{}",
        tool.as_str(),
        release.tag_name.replace('/', "-")
    ));
    // A previous run that died mid-extract would otherwise leave files here for
    // find_binary to pick up.
    tokio_fs::remove_dir_all(&staging_dir).await.ok();

    let result = install_staged(
        tool,
        release,
        asset,
        known_version,
        tool_dir,
        publisher,
        sidecar,
        &staging_dir,
    )
    .await;
    tokio_fs::remove_dir_all(&staging_dir).await.ok();
    result
}

#[allow(clippy::too_many_arguments)]
async fn install_staged(
    tool: Tool,
    release: &Release,
    asset: &Asset,
    known_version: Option<&Version>,
    tool_dir: &str,
    publisher: Option<&BucketPublisher>,
    sidecar: &mut Sidecar,
    staging_dir: &Path,
) -> Result<bool> {
    let staged = stage_release(tool, asset, staging_dir).await?;

    // Ask the binary. It is the authority on what a Scarb release ships, and its
    // answer is recorded whatever it turns out to be — including a version this
    // machine already has, or one the verifier will not accept. Recording those
    // is what stops the next check fetching this release to reach the same
    // conclusion an hour later.
    let version = match tool {
        Tool::Sozo => known_version
            .cloned()
            .ok_or_else(|| anyhow!("{} has no version in its tag", release.tag_name))?,
        Tool::Scarb => {
            let reported = extract_cairo_version(&staged).await?;
            if let Some(known) = known_version {
                if *known != reported {
                    warn!(
                        "{} ships Cairo {}, not {} as recorded; believing the binary",
                        release.tag_name, reported, known
                    );
                }
            }
            sidecar.record_release(&release.tag_name, reported.clone());
            reported
        }
    };

    if !tool.is_supported(&version) {
        info!(
            "{} turned out to be {}, which the verifier would not accept",
            release.tag_name, version
        );
        return Ok(false);
    }

    let name = tool.object_name(&version);
    let destination = format!("{}/{}", tool_dir, name);
    if Path::new(&destination).exists() {
        return Ok(false);
    }

    tokio_fs::rename(&staged, &destination)
        .await
        .with_context(|| format!("installing {}", destination))?;
    info!("Installed {} from {}", destination, release.tag_name);

    if let Some(publisher) = publisher {
        match publisher
            .publish_binary(tool, &name, Path::new(&destination))
            .await
        {
            // Recorded only once the object is actually in the bucket, so the
            // sidecar never claims something the bucket does not have.
            Ok(()) => sidecar.record_object(tool, &name, &version, release, asset),
            Err(err) => warn!(
                "{} is installed but could not be uploaded: {:?}",
                destination, err
            ),
        }
    }

    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn release(tag: &str, prerelease: bool) -> Release {
        Release {
            tag_name: tag.to_string(),
            assets: Vec::new(),
            prerelease,
        }
    }

    fn version(v: &str) -> Version {
        Version::parse(v).unwrap()
    }

    #[test]
    fn reads_the_version_out_of_every_tag_spelling() {
        assert_eq!(parse_version_from_tag("v2.20.1"), "2.20.1");
        assert_eq!(parse_version_from_tag("sozo/v1.8.1"), "1.8.1");
        assert_eq!(parse_version_from_tag(" 1.8.1 "), "1.8.1");
    }

    #[test]
    fn names_a_binary_the_way_the_verifier_will_ask_for_it() {
        // One spelling, whatever the version: the pre-2.8 binaries are named
        // the same way as the rest (crates/verification/src/scarb.rs:164
        // and :211).
        assert_eq!(
            Tool::Scarb.object_name(&version("2.6.3")),
            "scarb_cairo_v2.6.3"
        );
        assert_eq!(
            Tool::Scarb.object_name(&version("2.7.0")),
            "scarb_cairo_v2.7.0"
        );
        assert_eq!(
            Tool::Scarb.object_name(&version("2.8.2")),
            "scarb_cairo_v2.8.2"
        );
        assert_eq!(
            Tool::Scarb.object_name(&version("2.20.0")),
            "scarb_cairo_v2.20.0"
        );
        assert_eq!(Tool::Sozo.object_name(&version("1.8.1")), "sozo_v1.8.1");
    }

    #[test]
    fn resolves_a_dojo_tag_to_a_path_however_the_project_spelled_it() {
        // A Dojo tag is written by hand in a project's Scarb.toml, and the
        // support check accepts it with or without the `v`. Both spellings have
        // to reach the one binary the release check installed, which is what
        // pasting the tag into the filename did not do.
        for tag in ["v1.8.1", "1.8.1"] {
            let resolved = Version::parse(&parse_version_from_tag(tag)).unwrap();
            assert_eq!(
                Tool::Sozo.binary_path("/opt/app/binaries", &resolved),
                "/opt/app/binaries/sozo/sozo_v1.8.1"
            );
        }
    }

    #[test]
    fn builds_the_same_path_the_bucket_sync_writes_to() {
        // crates/server/src/binaries_manager_service.rs drops the bucket's
        // architecture segment to get here, so these two have to agree.
        assert_eq!(
            Tool::Scarb.binary_path("/opt/app/binaries", &version("2.6.3")),
            "/opt/app/binaries/scarb/scarb_cairo_v2.6.3"
        );
    }

    #[test]
    fn only_accepts_what_the_verifier_would_build_with() {
        // This is what replaced the hardcoded starting version: the reason to
        // skip a release is that nothing could ask for it.
        assert!(Tool::Scarb.is_supported(&version("2.6.3")));
        assert!(Tool::Scarb.is_supported(&version("2.8.2")));
        assert!(Tool::Scarb.is_supported(&version("2.20.0")));
        assert!(!Tool::Scarb.is_supported(&version("2.6.2")));
        assert!(!Tool::Scarb.is_supported(&version("2.8.0")));
        assert!(Tool::Sozo.is_supported(&version("1.0.1")));
        assert!(Tool::Sozo.is_supported(&version("1.8.7")));
        assert!(!Tool::Sozo.is_supported(&version("1.0.2")));
    }

    #[test]
    fn does_not_bother_fetching_releases_from_before_the_supported_range() {
        // A lower bound, not a version check: it saves fetching, unpacking and
        // running some thirty releases that cannot be shipping a Cairo the
        // verifier accepts. Everything at or above it is still asked.
        assert!(Tool::Scarb.is_too_old_to_consider(&version("2.5.4")));
        assert!(!Tool::Scarb.is_too_old_to_consider(&version("2.6.3")));
        assert!(!Tool::Scarb.is_too_old_to_consider(&version("2.20.1")));
        // Sozo needs no bound: its tag is the version, so support answers it.
        assert!(!Tool::Sozo.is_too_old_to_consider(&version("0.1.0")));
    }

    #[test]
    fn considers_releases_newest_first() {
        let candidates = installable_candidates(vec![
            release("v2.9.0", false),
            release("v2.20.1", false),
            release("v2.10.1", false),
        ]);
        let order: Vec<&str> = candidates
            .iter()
            .map(|(_, release)| release.tag_name.as_str())
            .collect();
        // Not lexicographic: sorting these as text would put 2.10.1 below 2.9.0
        // and hand the win to the older release.
        assert_eq!(order, ["v2.20.1", "v2.10.1", "v2.9.0"]);
    }

    #[test]
    fn drops_prereleases_however_they_are_marked() {
        let candidates = installable_candidates(vec![
            release("v2.17.0-rc.1", false), // tagged as one, not flagged
            release("v2.18.0", true),       // flagged as one, not tagged
            release("not-a-version", false),
            release("v2.18.0", false),
        ]);
        let order: Vec<&str> = candidates
            .iter()
            .map(|(_, release)| release.tag_name.as_str())
            .collect();
        assert_eq!(order, ["v2.18.0"]);
    }

    #[test]
    fn reads_the_cache_the_bucket_carries() {
        let document = json!({
            "releases": { "v2.20.1": "2.20.0", "v2.19.4": "2.19.4", "v9.9.9": "not a version" },
            "binaries": {}
        });
        let known = known_releases(&document);
        assert_eq!(known.get("v2.20.1"), Some(&version("2.20.0")));
        assert_eq!(known.get("v2.19.4"), Some(&version("2.19.4")));
        // One unreadable entry costs that release a lookup, not the whole map.
        assert_eq!(known.get("v9.9.9"), None);
        assert_eq!(known.len(), 2);
        assert!(known_releases(&json!({})).is_empty());
    }

    fn sidecar_with_updates() -> Sidecar {
        let mut sidecar = Sidecar {
            document: json!({}),
            known: BTreeMap::new(),
            added_releases: BTreeMap::new(),
            added_objects: Vec::new(),
        };
        sidecar.record_release("v2.21.0", version("2.21.0"));
        sidecar.record_object(
            Tool::Scarb,
            "scarb_cairo_v2.21.0",
            &version("2.21.0"),
            &release("v2.21.0", false),
            &Asset {
                browser_download_url: "https://example/scarb.tar.gz".to_string(),
                name: "scarb.tar.gz".to_string(),
            },
        );
        sidecar
    }

    #[test]
    fn merges_into_what_the_bucket_already_says() {
        // The document read back from the bucket may hold entries this run knows
        // nothing about — another machine's, or an older backfill's — and fields
        // this build has never heard of. Both have to survive the write.
        let mut document = json!({
            "tool": "scarb",
            "written_by": "backfill-binaries-bucket.sh",
            "something_new": ["from a later version"],
            "releases": { "v2.20.1": "2.20.0" },
            "binaries": { "scarb_cairo_v2.20.0": { "cairo": "2.20.0", "release": "v2.20.1" } },
            "newest_release": "v2.20.1"
        });
        sidecar_with_updates().apply_to(&mut document, Tool::Scarb, Some("x86_64"));

        assert_eq!(document["something_new"][0], json!("from a later version"));
        assert_eq!(document["releases"]["v2.20.1"], json!("2.20.0"));
        assert_eq!(document["releases"]["v2.21.0"], json!("2.21.0"));
        assert_eq!(
            document["binaries"]["scarb_cairo_v2.20.0"]["cairo"],
            json!("2.20.0")
        );
        assert_eq!(
            document["binaries"]["scarb_cairo_v2.21.0"],
            json!({
                "cairo": "2.21.0",
                "release": "v2.21.0",
                "asset": "https://example/scarb.tar.gz"
            })
        );
        assert_eq!(document["newest_release"], json!("v2.21.0"));
        assert_eq!(document["written_by"], json!("starknet-debugger-server"));
        assert_eq!(document["arch"], json!("x86_64"));
    }

    #[test]
    fn does_not_claim_the_bucket_is_empty_when_it_simply_did_not_look() {
        // A check that resolved releases but installed nothing has learned
        // nothing about which binaries the bucket holds — it never lists it.
        // Leaving `binaries` out says that; writing `{}` would say the bucket
        // is empty, which is a different and probably wrong claim.
        let mut sidecar = Sidecar {
            document: json!({}),
            known: BTreeMap::new(),
            added_releases: BTreeMap::new(),
            added_objects: Vec::new(),
        };
        sidecar.record_release("v2.20.1", version("2.20.0"));

        let mut document = json!({});
        sidecar.apply_to(&mut document, Tool::Scarb, Some("x86_64"));
        assert_eq!(document["releases"]["v2.20.1"], json!("2.20.0"));
        assert!(document.get("binaries").is_none());
        assert!(document.get("newest_release").is_none());
    }

    #[test]
    fn never_moves_newest_release_backwards() {
        // A machine installing an old version it happened to be missing has not
        // made the bucket less current.
        let mut document = json!({ "newest_release": "v2.30.0" });
        sidecar_with_updates().apply_to(&mut document, Tool::Scarb, Some("x86_64"));
        assert_eq!(document["newest_release"], json!("v2.30.0"));
    }

    #[test]
    fn writes_a_sidecar_where_there_was_none() {
        let mut document = json!(null);
        sidecar_with_updates().apply_to(&mut document, Tool::Sozo, Some("arm64"));
        assert_eq!(document["tool"], json!("sozo"));
        assert_eq!(document["newest_release"], json!("v2.21.0"));
        assert!(document["written_at"].is_string());
    }

    #[tokio::test]
    #[ignore = "hits the GitHub API"]
    async fn lists_every_release_and_not_just_the_newest_page() {
        // The only claim here that a unit test cannot make. Run with:
        //   cargo test -p verification -- --ignored lists_every_release
        //
        // Unpaginated, this call answered with 30 — which is how a toolchain
        // that had scrolled off that page became unreachable from upstream as
        // well as absent from disk.
        let releases = list_all_releases("software-mansion/scarb")
            .await
            .expect("the release list should be readable");
        assert!(
            releases.len() > 100,
            "only {} releases; pagination is not reaching past the first page",
            releases.len()
        );
        assert!(releases.iter().any(|r| r.tag_name == "v2.6.5"));
    }
}
