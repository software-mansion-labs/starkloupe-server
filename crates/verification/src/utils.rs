use crate::db::fetch_verification_id_and_status;
use crate::EVerificationStatus;
use anyhow::Result;
use sqlx::{Pool, Postgres};
use std::fs;
use std::io::Write;
use std::path::PathBuf;
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
    pub dojo_alpha_version: Option<u8>,
    pub dojo_namespace_name: Option<String>,
}

pub fn read_manifest(tmp_dir: &PathBuf) -> Result<Manifest> {
    let scarb_config_file = tmp_dir.join("Scarb.toml");
    let dojo_config_file = tmp_dir.join("dojo_dev.toml");

    let scarb_config_contents = match fs::read_to_string(scarb_config_file) {
        Ok(c) => c,
        Err(e) => {
            error!("Failed to read Scarb.toml: {}", e);
            return Err(anyhow::anyhow!("Failed to read Scarb.toml: {}", e));
        }
    };

    // Parse the string as TOML
    let scarb_config_toml = match scarb_config_contents.parse::<toml::Value>() {
        Ok(parsed) => parsed,
        Err(e) => {
            error!("Failed to parse Scarb.toml: {}", e);
            return Err(anyhow::anyhow!("Failed to parse Scarb.toml: {}", e));
        }
    };

    let package_name = match scarb_config_toml
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
    let has_dojo_target = scarb_config_toml
        .get("target")
        .and_then(|t| t.get("dojo"))
        .is_some();

    // Check for dojo dependency and get the tag value
    let dojo_tag = scarb_config_toml
        .get("dependencies")
        .and_then(|d| d.get("dojo"))
        .and_then(|dojo| dojo.get("tag"))
        .and_then(toml::Value::as_str)
        .map(|tag| tag.to_string());

    // Extract the number if the tag starts with "v1.0.0-alpha"
    let dojo_alpha_version: Option<u8> = dojo_tag.as_deref().and_then(|tag| {
        if tag.starts_with("v1.0.0-alpha") {
            tag.strip_prefix("v1.0.0-alpha.")
                .and_then(|s| s.parse::<u8>().ok())
        } else {
            None
        }
    });

    // Read dojo_dev.toml and extract the namespace.default value
    let dojo_namespace_name = match fs::read_to_string(dojo_config_file) {
        Ok(contents) => {
            let dojo_config_toml = match contents.parse::<toml::Value>() {
                Ok(parsed) => parsed,
                Err(e) => {
                    error!("Failed to parse dojo_dev.toml: {}", e);
                    return Err(anyhow::anyhow!("Failed to parse dojo_dev.toml: {}", e));
                }
            };

            dojo_config_toml
                .get("namespace")
                .and_then(|ns| ns.get("default"))
                .and_then(toml::Value::as_str)
                .map(|s| s.to_string())
        }
        Err(_) => None,
    };

    Ok(Manifest {
        package_name: package_name.to_string(),
        has_dojo_target,
        dojo_alpha_version,
        dojo_namespace_name,
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
