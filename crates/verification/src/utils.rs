use crate::db::fetch_verification_id_and_status;
use crate::EVerificationStatus;
use anyhow::Result;
use sqlx::{Pool, Postgres};
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::{collections::HashMap, fs::File};
use tracing::{error, info};

pub fn create_files_from_map(
    source_code: &HashMap<String, String>,
    dir_path: &PathBuf,
) -> Result<()> {
    for (path, content) in source_code {
        let mut full_path = dir_path.clone();
        full_path.push(path);

        if let Some(dir) = full_path.parent() {
            fs::create_dir_all(dir)?;
        }
        let mut file = File::create(&full_path)?;
        file.write_all(content.as_bytes())?;
    }
    Ok(())
}

#[derive(Debug)]
pub struct Manifest {
    pub package_name: String,
    pub has_dojo_target: bool,
}

pub fn read_manifest(path: &Path) -> Result<Manifest> {
    let contents = match fs::read_to_string(path) {
        Ok(c) => c,
        Err(e) => {
            error!("Failed to read Scarb.toml: {}", e);
            return Err(anyhow::anyhow!("Failed to read Scarb.toml: {}", e));
        }
    };

    // Parse the string as TOML
    let toml = match contents.parse::<toml::Value>() {
        Ok(parsed) => parsed,
        Err(e) => {
            error!("Failed to parse Scarb.toml: {}", e);
            return Err(anyhow::anyhow!("Failed to parse Scarb.toml: {}", e));
        }
    };

    let package_name = match toml
        .get("package")
        .and_then(|p| p.get("name"))
        .and_then(toml::Value::as_str)
    {
        Some(name) => name,
        None => {
            error!("Package name not found in Scarb.toml");
            return Err(anyhow::anyhow!("No package name found"));
        }
    };

    // Check for [target.dojo]
    let has_dojo_target = toml.get("target").and_then(|t| t.get("dojo")).is_some();

    Ok(Manifest {
        package_name: package_name.to_string(),
        has_dojo_target,
    })
}

pub async fn check_verification_status(
    db_pool: &Pool<Postgres>,
    class_hash: String,
    chain_id: Option<String>,
) -> Result<()> {
    let result =
        fetch_verification_id_and_status(db_pool, class_hash.clone(), chain_id.unwrap_or_default())
            .await?;

    match result {
        Some((existing_id, status)) => match status {
            EVerificationStatus::Pending => {
                return Err(anyhow::anyhow!(
                        "Verification is in progress. Please check the status at: https://api.walnut.dev/v1/verification/{}/status.",
                        existing_id
                    ));
            }
            EVerificationStatus::Success => {
                return Err(anyhow::anyhow!(
                        "Verification is completed successfully. You can access the class details at: https://api.walnut.dev/v1/classes/{}.",
                        class_hash
                    ));
            }
            EVerificationStatus::Failed => {
                info!("Verification failed. You can start a new verification.");
            }
        },
        None => {
            info!("No existing verification status found. You can proceed with verification.");
        }
    }

    Ok(())
}
