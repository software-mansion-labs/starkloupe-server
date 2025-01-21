use std::env::consts::ARCH;
use std::fs;
use std::fs::File;
use std::io::Write;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::time::Duration;
use aws_sdk_s3::Client;
use clokwerk::{AsyncScheduler, TimeUnits};
use tokio::spawn;
use tracing::{error, info};
use verification::scarb_download_scheduler::check_periodically_scarb_updates;


pub async fn download_custom_scarb_binaries(s3_client: &Client) -> Result<(), Box<dyn std::error::Error>> {
    let architecture = ARCH;
    let s3_folder = match architecture {
        "x86_64" => "x86_64",
        "aarch64" | "arm" => "arm64",
        _ => return Err(Box::from(format!("Unsupported architecture: {}", architecture))),
    };
    download_binary(&s3_client, format!("sozo/{s3_folder}/sozo_v1.0.1").as_str()).await?;
    download_binary(&s3_client, format!("sozo/{s3_folder}/sozo_v1.0.12").as_str()).await?;
    download_binary(&s3_client, format!("scarb/{s3_folder}/scarb_cairo_v_2_6_3").as_str()).await?;
    download_binary(&s3_client, format!("scarb/{s3_folder}/scarb_cairo_v_2_6_4").as_str()).await?;
    download_binary(&s3_client, format!("scarb/{s3_folder}/scarb_cairo_v_2_7_0").as_str()).await?;
    download_binary(&s3_client, format!("scarb/{s3_folder}/scarb_cairo_v2.8.2").as_str()).await?;
    download_binary(&s3_client, format!("scarb/{s3_folder}/scarb_cairo_v2.8.4").as_str()).await?;
    download_binary(&s3_client, format!("scarb/{s3_folder}/scarb_cairo_v2.8.5").as_str()).await?;
    Ok(())
}

// downloads the binary from the S3 bucket and saves it to the local directory, gives the executable permissions
async fn download_binary(
    s3_client: &Client,
    object_key: &str,
) -> Result<(), Box<dyn std::error::Error>> {
    let bucket_name = std::env::var("BINARIES_S3_BUCKET_NAME").unwrap_or("./binaries".to_string());
    let binaries_save_directory_path = std::env::var("BINARIES_SAVE_DIRECTORY_PATH").unwrap_or("".to_string());
    let local_file_path = format!("{}/{}", &binaries_save_directory_path, &object_key.replace(format!("/{}/", ARCH).as_str(), "/"));
    // Check if the file already exists
    let path = Path::new(&local_file_path);
    if path.exists() {
        info!("File already exists (skipping download): {}", local_file_path);
        return Ok(()); // Exit early if the file exists
    }
    info!("Downloading object: {}/{}", bucket_name, object_key);

    // Fetch the object from the S3 bucket
    let resp = s3_client
        .get_object()
        .bucket(bucket_name)
        .key(object_key)
        .send()
        .await?;

    // Ensure the directory exists
    if let Some(parent_dir) = std::path::Path::new(&local_file_path).parent() {
        fs::create_dir_all(parent_dir).expect("Failed to create parent directories");
    }
    let mut file = File::create(&local_file_path).expect(format!("Failed to create file: {}", local_file_path).as_str());

    // Stream the object content to the file
    let data = resp.body.collect().await?;
    file.write_all(&data.into_bytes())
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
    let interval: u32 = std::env::var("SCARB_RUN_SCHEDULER_INTERVAL_MINUTES")
        .unwrap_or_else(|_| "60".to_string())
        .parse::<u32>()
        .unwrap();
    let mut scheduler = AsyncScheduler::with_tz(chrono::Utc);
    info!("Starting scarb binaries downloader scheduler. Checking every: {} minutes", interval);
    scheduler
        .every(interval.minutes())
        .run(|| async {
            info!("Starting scarb update check");
            let scarb_repo = std::env::var("SCARB_GITHUB_REPO_NAME")
                .expect("Environment variable SCARB_GITHUB_REPO_NAME is not set");
            let versioning_file_name = std::env::var("SCARB_LATEST_VERSION_FILE_NAME")
                .expect("Environment variable SCARB_LATEST_VERSION_FILE_NAME is not set");
            if let Err(err) = check_periodically_scarb_updates(scarb_repo.as_str(), versioning_file_name.as_str()).await {
                error!("Error in scarb update check: {:?}", err);
            } else {
                info!("Finished scarb update check");
            }
        });
    spawn(async move {
        loop {
            scheduler.run_pending().await;
            tokio::time::sleep(Duration::from_secs(60)).await;
        }
    });
}
