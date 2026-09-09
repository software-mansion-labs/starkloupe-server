use aws_sdk_s3::Client;
use chrono::Utc;
use clokwerk::{AsyncScheduler, TimeUnits};
use lazy_regex::regex_captures;
use std::fs;
use std::fs::File;
use std::io::Write;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::time::Duration;
use tokio::spawn;
use tracing::{error, info, warn};
use verification::binary_names::{bucket_arch_folder, Tool};
use verification::scarb_and_dojo_download_scheduler::{
    check_periodically_scarb_updates, check_periodically_sozo_updates,
};

/// Where a bucket object belongs on disk: `scarb/x86_64/scarb_cairo_v2.10.1`
/// becomes `<BINARIES_SAVE_DIRECTORY_PATH>/scarb/scarb_cairo_v2.10.1`. The
/// architecture segment exists only in the bucket - the verifier looks under
/// `<dir>/<tool>/<name>`.
///
/// The name may be an install marker, which lives one directory deeper (in
/// `binary_names::INSTALLED_MARKER_DIR`); the rest of the path is carried over
/// as it is, so a marker restores where `is_installed` reads it from.
///
/// Returns `None` for keys of any other shape: other architectures, deeper
/// nesting, and the empty folder markers a listing can contain.
fn local_path_for(
    object_key: &str,
    arch_folder: &str,
    binaries_save_directory_path: &str,
) -> Option<String> {
    let (_, tool, arch, name) =
        regex_captures!(r"^([^/]+)/([^/]+)/(\.installed/[^/]+|[^/]+)$", object_key)?;
    (arch == arch_folder).then(|| format!("{binaries_save_directory_path}/{tool}/{name}"))
}

/// Every object in the binaries bucket.
async fn list_bucket_objects(
    s3_client: &Client,
) -> Result<Vec<String>, Box<dyn std::error::Error>> {
    let bucket_name = std::env::var("BINARIES_S3_BUCKET_NAME").unwrap_or("./binaries".to_string());
    let mut pages = s3_client
        .list_objects_v2()
        .bucket(&bucket_name)
        .into_paginator()
        .send();

    let mut keys: Vec<String> = Vec::new();
    while let Some(page) = pages.next().await {
        keys.extend(
            page?
                .contents()
                .iter()
                .filter_map(|object| object.key().map(str::to_string)),
        );
    }

    Ok(keys)
}

/// Populate the local toolchain directory from the binaries bucket.
///
/// This will pull the objects the bucket holds for this architecture, making no
/// assumptions about which those are. Whatever is missing is pulled from GitHub
/// releases by the scheduler started right after this, which caches what it
/// installs back into the bucket - so each release is fetched from GitHub once
/// across all machines, and a cold start after that restores it from here.
pub async fn download_scarb_and_sozo_binaries_from_s3(
    s3_client: &Client,
) -> Result<(), Box<dyn std::error::Error>> {
    let arch_folder = bucket_arch_folder()?;
    let binaries_save_directory_path =
        std::env::var("BINARIES_SAVE_DIRECTORY_PATH").unwrap_or("".to_string());

    let keys = list_bucket_objects(s3_client).await?;
    info!("Bucket holds {} objects", keys.len());

    let mut downloaded = 0usize;
    let mut already_present = 0usize;
    let mut other_architecture = 0usize;

    for key in keys {
        if local_path_for(&key, arch_folder, &binaries_save_directory_path).is_none() {
            other_architecture += 1;
            continue;
        }
        if download_binary(s3_client, &key).await? {
            downloaded += 1;
        } else {
            already_present += 1;
        }
    }

    if downloaded == 0 && already_present == 0 {
        warn!(
            "The binaries bucket holds nothing for {} — verification builds will fail until it is backfilled",
            arch_folder
        );
    } else {
        info!(
            "Toolchains from the bucket for {}: {} downloaded, {} already on disk, {} for other architectures",
            arch_folder, downloaded, already_present, other_architecture
        );
    }

    Ok(())
}

// Downloads the binary from the bucket, saves it to the local directory and
// gives it executable permissions. Returns whether anything was downloaded.
async fn download_binary(
    s3_client: &Client,
    object_key: &str,
) -> Result<bool, Box<dyn std::error::Error>> {
    let bucket_name = std::env::var("BINARIES_S3_BUCKET_NAME").unwrap_or("./binaries".to_string());
    let binaries_save_directory_path =
        std::env::var("BINARIES_SAVE_DIRECTORY_PATH").unwrap_or("".to_string());

    let local_file_path = match local_path_for(
        object_key,
        bucket_arch_folder()?,
        &binaries_save_directory_path,
    ) {
        Some(path) => path,
        None => {
            warn!(
                "Ignoring unexpected object key in the bucket: {}",
                object_key
            );
            return Ok(false);
        }
    };

    // Check if the file already exists
    if Path::new(&local_file_path).exists() {
        info!(
            "File already exists (skipping download): {}",
            local_file_path
        );
        return Ok(false); // Exit early if the file exists
    }
    info!("Downloading object: {}/{}", bucket_name, object_key);

    // Fetch the object from the S3 bucket
    let resp = match s3_client
        .get_object()
        .bucket(bucket_name)
        .key(object_key)
        .send()
        .await
    {
        Ok(resp) => resp,
        Err(err) => {
            // If the object simply isn't in the bucket (404 NoSuchKey), don't crash the
            // server — just log it and skip this binary. Other errors (auth, network,
            // wrong region/endpoint) still bubble up, since those mean S3 is misconfigured.
            if err
                .as_service_error()
                .map(|e| e.is_no_such_key())
                .unwrap_or(false)
            {
                warn!("Binary not found in S3 (skipping download): {}", object_key);
                return Ok(false);
            }
            return Err(err.into());
        }
    };
    // Ensure the directory exists
    if let Some(parent_dir) = Path::new(&local_file_path).parent() {
        fs::create_dir_all(parent_dir)?;
    }

    // Write beside the destination and rename into place: the existence check
    // above is the only thing gating a re-download, so a file left half-written
    // by an interrupted startup would be taken for an installed toolchain.
    let partial_file_path = format!("{}.partial", local_file_path);
    let mut file = File::create(&partial_file_path)?;

    // Stream the object content to the file
    let data = resp.body.collect().await?;
    file.write_all(&data.into_bytes())?;

    let mut permissions = fs::metadata(&partial_file_path)?.permissions();
    permissions.set_mode(0o755); // rwxr-xr-x
    fs::set_permissions(&partial_file_path, permissions)?;
    fs::rename(&partial_file_path, &local_file_path)?;

    info!("Object downloaded successfully to: {}", local_file_path);

    Ok(true)
}

pub async fn start_github_scarb_binaries_downloader_scheduler(s3_client: Client) {
    start_downloader_scheduler(
        Tool::Scarb,
        "SCARB_GITHUB_REPO_NAME".to_string(),
        "SCARB_RUN_SCHEDULER_INTERVAL_MINUTES".to_string(),
        s3_client,
    )
    .await;
}

pub async fn start_github_dojo_binaries_downloader_scheduler(s3_client: Client) {
    start_downloader_scheduler(
        Tool::Sozo,
        "DOJO_GITHUB_REPO_NAME".to_string(),
        "DOJO_RUN_SCHEDULER_INTERVAL_MINUTES".to_string(),
        s3_client,
    )
    .await;
}

// 1. Runs immidiately after app startup
// 2. Then runs every X minutes (60 by default)
pub async fn start_downloader_scheduler(
    tool: Tool,
    repo_env_var: String,
    interval_env_var: String,
    s3_client: Client,
) {
    let interval: u32 = std::env::var(&interval_env_var)
        .unwrap_or_else(|_| "60".to_string())
        .parse::<u32>()
        .unwrap();

    let mut scheduler = AsyncScheduler::with_tz(Utc);
    info!(
        "Starting {} binaries downloader scheduler. Checking every: {} minutes",
        tool, &interval
    );

    run_task(tool, repo_env_var.as_ref(), &s3_client).await;

    scheduler.every(interval.minutes()).run(move || {
        let repo_env_var = repo_env_var.clone();
        let s3_client = s3_client.clone();
        async move {
            run_task(tool, repo_env_var.as_ref(), &s3_client).await;
        }
    });

    spawn(async move {
        loop {
            scheduler.run_pending().await;
            tokio::time::sleep(Duration::from_secs(60)).await;
        }
    });
}

async fn run_task(tool: Tool, repo_env_var: &str, s3_client: &Client) {
    info!("Starting {} update check", tool);

    let repo = match std::env::var(repo_env_var) {
        Ok(value) => value,
        Err(_) => {
            error!("Environment variable {} is not set", repo_env_var);
            return;
        }
    };

    let res = match tool {
        Tool::Scarb => check_periodically_scarb_updates(repo.as_ref(), s3_client).await,
        Tool::Sozo => check_periodically_sozo_updates(repo.as_ref(), s3_client).await,
    };

    match res {
        Ok(_) => info!("Finished {} update check", tool),
        Err(err) => error!("Error in {} update check: {:?}", tool, err),
    }
}

#[cfg(test)]
mod tests {
    use super::local_path_for;
    use verification::binary_names::{installed_marker_relative_path, Tool};

    #[test]
    fn maps_a_bucket_key_to_the_path_the_verifier_reads() {
        assert_eq!(
            local_path_for(
                "scarb/x86_64/scarb_cairo_v2.10.1",
                "x86_64",
                "/opt/app/binaries"
            ),
            Some("/opt/app/binaries/scarb/scarb_cairo_v2.10.1".to_string())
        );
    }

    #[test]
    fn strips_the_arch_segment_whatever_it_is_called() {
        // The bucket says "arm64" where Rust says "aarch64". The segment is
        // dropped by position rather than by name so the two cannot drift —
        // matching on ARCH used to leave arm binaries in a directory the
        // verifier never looks in.
        assert_eq!(
            local_path_for("sozo/arm64/sozo_v1.0.1", "arm64", "/opt/app/binaries"),
            Some("/opt/app/binaries/sozo/sozo_v1.0.1".to_string())
        );
    }

    #[test]
    fn restores_an_install_marker_into_the_directory_it_is_read_from() {
        // The marker keyed by release tag is cached with the binaries, and has
        // to land where `is_installed` looks for it. Building the expected path
        // from `installed_marker_path` ties this to the naming, so the two
        // cannot drift - the key here is one directory deeper than a binary.
        assert_eq!(
            local_path_for(
                &Tool::Scarb.bucket_key("x86_64", &installed_marker_relative_path("v2.12.0")),
                "x86_64",
                "/opt/app/binaries"
            ),
            Some(Tool::Scarb.installed_marker_path("/opt/app/binaries", "v2.12.0"))
        );
    }

    #[test]
    fn rejects_anything_that_is_not_tool_arch_name() {
        assert_eq!(local_path_for("scarb/x86_64/", "x86_64", "/binaries"), None);
        assert_eq!(local_path_for("scarb/x86_64", "x86_64", "/binaries"), None);
        assert_eq!(
            local_path_for("scarb/x86_64/nested/scarb", "x86_64", "/binaries"),
            None
        );
        // Only the marker directory goes one level deeper, and only with a
        // file in it.
        assert_eq!(
            local_path_for("scarb/x86_64/.installed/", "x86_64", "/binaries"),
            None
        );
        assert_eq!(
            local_path_for("scarb/x86_64/.installed/a/b", "x86_64", "/binaries"),
            None
        );
        assert_eq!(local_path_for("", "x86_64", "/binaries"), None);
    }

    #[test]
    fn ignores_objects_belonging_to_another_architecture() {
        // The whole bucket is listed now, so this filter is the only thing
        // keeping an arm build off an x86 machine.
        assert_eq!(
            local_path_for("scarb/arm64/scarb_cairo_v2.10.1", "x86_64", "/binaries"),
            None
        );
    }
}
