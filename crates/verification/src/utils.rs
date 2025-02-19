use anyhow::Result;
use fs_extra::dir::{copy, CopyOptions};
use libc::{rlimit, setrlimit, RLIMIT_AS, RLIMIT_CPU};
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
pub fn move_failed_verification_to_failed_tmp(
    tmp_dir: &PathBuf,
    verification_id: &Uuid,
) -> Result<()> {
    let failed_tmp_dir = PathBuf::from(format!("tmp/failed-verification/{}", verification_id));

    error!(
        "Failed to verify classes - moving {} to {} for further investigation.",
        &tmp_dir.display(),
        &failed_tmp_dir.display(),
    );
    if !failed_tmp_dir.exists() {
        fs::create_dir_all(&failed_tmp_dir)?;
    }

    let mut options = CopyOptions::new();
    options.copy_inside = true;

    copy(tmp_dir, failed_tmp_dir, &options)?;

    fs::remove_dir_all(tmp_dir)?;

    Ok(())
}

pub fn remove_walnut_debug_from_scarb(source_code: &mut HashMap<String, String>) {
    if let Some(scarb_toml_contents) = source_code.get("Scarb.toml") {
        if let Ok(mut scarb_toml_parsed) = scarb_toml_contents.parse::<toml::Value>() {
            if let Some(profile_table) = scarb_toml_parsed
                .get_mut("profile")
                .and_then(toml::Value::as_table_mut)
            {
                profile_table.remove("walnut-debug");
                match toml::to_string(&scarb_toml_parsed) {
                    Ok(updated_scarb_config) => {
                        source_code.insert("Scarb.toml".to_string(), updated_scarb_config);
                    }
                    Err(e) => {
                        error!("Failed to serialize and update Scarb.toml: {}", e);
                    }
                }
            }
        }
    }
}

#[cfg(target_os = "linux")]
fn set_limit_linux(cpu_limit: u64) -> std::io::Result<()> {
    let cpu_limit_struct = rlimit {
        rlim_cur: cpu_limit,
        rlim_max: cpu_limit,
    };

    // Set RLIMIT_CPU to limit CPU time (seconds) the process can use
    let cpu_result = unsafe { setrlimit(RLIMIT_CPU, &cpu_limit_struct) };
    if cpu_result != 0 {
        return Err(std::io::Error::new(
            std::io::ErrorKind::Other,
            "Linux setrlimit (RLIMIT_CPU) failed".to_string(),
        ));
    }

    Ok(())
}

#[cfg(target_os = "macos")]
fn set_limit_macos(_cpu_limit: u64) -> std::io::Result<()> {
    // No memory or CPU limits set on macOS
    Ok(())
}

pub fn set_limits(cpu_limit: u64) -> std::io::Result<()> {
    #[cfg(target_os = "linux")]
    return set_limit_linux(cpu_limit);

    #[cfg(target_os = "macos")]
    return set_limit_macos(cpu_limit);
}
