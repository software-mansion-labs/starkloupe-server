use anyhow::Result;
use std::fs;
use std::io::Write;
use std::path::PathBuf;
use std::{collections::HashMap, fs::File};
use tracing::error;
use uuid::Uuid;

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
pub fn read_file(file_path: &PathBuf) -> Result<String> {
    fs::read_to_string(file_path).map_err(|e| {
        let error_message = format!("Failed to read file {:?}: {:?}", file_path, e);
        error!("{}", error_message);
        anyhow::anyhow!(error_message)
    })
}

pub fn deserialize_json<T: serde::de::DeserializeOwned>(
    json_str: &str,
    context: &str,
) -> Result<T> {
    serde_json::from_str(json_str).map_err(|e| {
        let error_message = format!("Failed to deserialize {}: {:?}", context, e);
        error!("{}", error_message);
        anyhow::anyhow!(error_message)
    })
}

pub fn create_temp_directory() -> Result<PathBuf> {
    let tmp_dir = PathBuf::from("tmp/verification").join(Uuid::new_v4().to_string());
    fs::create_dir_all(&tmp_dir)?;
    Ok(tmp_dir)
}

// Failed verifications data for further investigation.
// There is no auto removal from this location.
pub fn move_failed_verification_to_failed_tmp(tmp_dir: &PathBuf, e: &anyhow::Error) -> Result<()> {
    let failed_tmp_dir = PathBuf::from("tmp/failed-verification").join(Uuid::new_v4().to_string());

    error!(
        "Failed to verify classes - moving {} to {} for further investigation. Error: {:#}",
        &tmp_dir.display(),
        &failed_tmp_dir.display(),
        e
    );
    if !failed_tmp_dir.exists() {
        fs::create_dir_all(&failed_tmp_dir)?;
    }
    fs::rename(tmp_dir, &failed_tmp_dir)?;
    Ok(())
}
