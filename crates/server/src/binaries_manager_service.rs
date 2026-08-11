use chrono::Utc;
use clokwerk::{AsyncScheduler, TimeUnits};
use google_cloud_storage::client::Storage;
use std::env::consts::ARCH;
use std::fs;
use std::fs::File;
use std::io::Write;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::time::Duration;
use tokio::spawn;
use tracing::{error, info, warn};
use verification::scarb_and_dojo_download_scheduler::{
    check_periodically_scarb_updates, check_periodically_sozo_updates,
};

pub async fn download_scarb_and_sozo_binaries_from_gcs(
    gcs_client: &Storage,
) -> Result<(), Box<dyn std::error::Error>> {
    let architecture = ARCH;
    let gcs_folder = match architecture {
        "x86_64" => "x86_64",
        "aarch64" | "arm" => "arm64",
        _ => {
            return Err(Box::from(format!(
                "Unsupported architecture: {}",
                architecture
            )))
        }
    };
    download_binary(
        gcs_client,
        format!("sozo/{gcs_folder}/sozo_v1.0.1").as_str(),
    )
    .await?;
    download_binary(
        gcs_client,
        format!("sozo/{gcs_folder}/sozo_v1.0.12").as_str(),
    )
    .await?;
    download_binary(
        gcs_client,
        format!("scarb/{gcs_folder}/scarb_cairo_v_2_6_3").as_str(),
    )
    .await?;
    download_binary(
        gcs_client,
        format!("scarb/{gcs_folder}/scarb_cairo_v_2_6_4").as_str(),
    )
    .await?;
    download_binary(
        gcs_client,
        format!("scarb/{gcs_folder}/scarb_cairo_v_2_7_0").as_str(),
    )
    .await?;
    download_binary(
        gcs_client,
        format!("scarb/{gcs_folder}/scarb_cairo_v2.8.2").as_str(),
    )
    .await?;
    download_binary(
        gcs_client,
        format!("scarb/{gcs_folder}/scarb_cairo_v2.8.4").as_str(),
    )
    .await?;
    download_binary(
        gcs_client,
        format!("scarb/{gcs_folder}/scarb_cairo_v2.8.5").as_str(),
    )
    .await?;
    Ok(())
}

// downloads the binary from the GCS bucket and saves it to the local directory, gives the executable permissions
async fn download_binary(
    gcs_client: &Storage,
    object_key: &str,
) -> Result<(), Box<dyn std::error::Error>> {
    let bucket_name = std::env::var("BINARIES_GCS_BUCKET_NAME").unwrap_or("./binaries".to_string());
    let binaries_save_directory_path =
        std::env::var("BINARIES_SAVE_DIRECTORY_PATH").unwrap_or("".to_string());
    let local_file_path = format!(
        "{}/{}",
        &binaries_save_directory_path,
        &object_key.replace(format!("/{}/", ARCH).as_str(), "/")
    );
    // Check if the file already exists
    let path = Path::new(&local_file_path);
    if path.exists() {
        info!(
            "File already exists (skipping download): {}",
            local_file_path
        );
        return Ok(()); // Exit early if the file exists
    }
    info!("Downloading object: {}/{}", bucket_name, object_key);

    // Fetch the object from the GCS bucket
    let bucket = format!("projects/_/buckets/{bucket_name}");
    let mut resp = match gcs_client.read_object(bucket, object_key).send().await {
        Ok(resp) => resp,
        Err(err) => {
            // If the object simply isn't in the bucket (404), don't crash the
            // server — just log it and skip this binary. Other errors (auth, network,
            // misconfigured bucket) still bubble up, since those mean GCS is misconfigured.
            if err.http_status_code() == Some(404) {
                warn!(
                    "Binary not found in GCS (skipping download): {}",
                    object_key
                );
                return Ok(());
            }
            return Err(err.into());
        }
    };
    // Ensure the directory exists
    if let Some(parent_dir) = std::path::Path::new(&local_file_path).parent() {
        fs::create_dir_all(parent_dir).expect("Failed to create parent directories");
    }
    let mut file = File::create(&local_file_path)
        .unwrap_or_else(|_| panic!("Failed to create file: {}", local_file_path));

    // Stream the object content to the file
    let mut data = Vec::new();
    while let Some(chunk) = resp.next().await.transpose()? {
        data.extend_from_slice(&chunk);
    }
    file.write_all(&data)
        .expect("Failed to write object data to file");

    let metadata = fs::metadata(&local_file_path)?;
    let mut permissions = metadata.permissions();
    permissions.set_mode(0o755); // rwxr-xr-x
    fs::set_permissions(&local_file_path, permissions)
        .expect("Failed to set executable permissions");

    info!("Object downloaded successfully to: {}", local_file_path);

    Ok(())
}

pub async fn start_github_scarb_binaries_downloader_scheduler() {
    start_downloader_scheduler(
        "scarb".to_string(),
        "SCARB_GITHUB_REPO_NAME".to_string(),
        "SCARB_LATEST_VERSION_FILE_NAME".to_string(),
        "SCARB_RUN_SCHEDULER_INTERVAL_MINUTES".to_string(),
    )
    .await;
}

pub async fn start_github_dojo_binaries_downloader_scheduler() {
    start_downloader_scheduler(
        "sozo".to_string(),
        "DOJO_GITHUB_REPO_NAME".to_string(),
        "DOJO_LATEST_VERSION_FILE_NAME".to_string(),
        "DOJO_RUN_SCHEDULER_INTERVAL_MINUTES".to_string(),
    )
    .await;
}

// 1. Runs immidiately after app startup
// 2. Then runs every X minutes (60 by default)
pub async fn start_downloader_scheduler(
    tool_name: String,
    repo_env_var: String,
    versioning_file_name_env_var: String,
    interval_env_var: String,
) {
    let interval: u32 = std::env::var(&interval_env_var)
        .unwrap_or_else(|_| "60".to_string())
        .parse::<u32>()
        .unwrap();

    let mut scheduler = AsyncScheduler::with_tz(Utc);
    info!(
        "Starting {} binaries downloader scheduler. Checking every: {} minutes",
        &tool_name, &interval
    );

    run_task(
        tool_name.as_ref(),
        repo_env_var.as_ref(),
        versioning_file_name_env_var.as_ref(),
    )
    .await;

    scheduler.every(interval.minutes()).run(move || {
        let name = tool_name.clone();
        let repo_env_var = repo_env_var.clone();
        let versioning_file_name_env_var = versioning_file_name_env_var.clone();
        async move {
            run_task(
                name.as_ref(),
                repo_env_var.as_ref(),
                versioning_file_name_env_var.as_ref(),
            )
            .await;
        }
    });

    spawn(async move {
        loop {
            scheduler.run_pending().await;
            tokio::time::sleep(Duration::from_secs(60)).await;
        }
    });
}

async fn run_task(tool_name: &str, repo_env_var: &str, versioning_file_name_env_var: &str) {
    info!("Starting {} update check", tool_name);

    let repo = match std::env::var(repo_env_var) {
        Ok(value) => value,
        Err(_) => {
            error!("Environment variable {} is not set", repo_env_var);
            return;
        }
    };

    let versioning_file_name = match std::env::var(versioning_file_name_env_var) {
        Ok(value) => value,
        Err(_) => {
            error!(
                "Environment variable {} is not set",
                versioning_file_name_env_var
            );
            return;
        }
    };

    let res = match tool_name {
        "scarb" => {
            check_periodically_scarb_updates(repo.as_ref(), versioning_file_name.as_ref()).await
        }
        "sozo" => {
            check_periodically_sozo_updates(repo.as_ref(), versioning_file_name.as_ref()).await
        }
        _ => {
            error!("Unknown tool name: {}", &tool_name);
            return;
        }
    };

    match res {
        Ok(_) => info!("Finished {} update check", tool_name),
        Err(err) => error!("Error in {} update check: {:?}", tool_name, err),
    }
}
