use crate::utils::set_limits;
use anyhow::Result;
use std::os::unix::process::CommandExt;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::{env, fs};
use tracing::error;

pub fn run_sozo_build_for_profile(tmp_dir: &PathBuf, sozo_path: &str, profile: &str) -> Result<()> {
    let memory_limit: u64 = std::env::var("BUILD_MEMORY_LIMIT")
        .unwrap_or("8589934592".to_string())
        .parse::<u64>()?;

    let cpu_limit: u64 = std::env::var("BUILD_CPU_LIMIT")
        .unwrap_or("120".to_string())
        .parse::<u64>()?;

    let absolute_path = fs::canonicalize(sozo_path)?;

    let scarb_cache_dir = env::current_dir()?.join(".cache/scarb");
    let scarb_cache_dir_str = scarb_cache_dir.to_str().ok_or_else(|| {
        error!("Error converting cache directory to string");
        anyhow::anyhow!("Failed to convert cache dir to string")
    })?;

    let child_result = unsafe {
        Command::new(absolute_path)
            .env("SCARB_CACHE", scarb_cache_dir_str)
            .arg("build")
            .arg("--profile")
            .arg(profile)
            .current_dir(tmp_dir)
            .stdout(Stdio::inherit())
            .stderr(Stdio::inherit())
            .pre_exec(move || {
                if let Err(e) = set_limits(memory_limit, cpu_limit) {
                    error!("Failed to set memory limit: {:?}", e);
                    return Err(std::io::Error::new(
                        std::io::ErrorKind::Other,
                        "Failed to set memory limit",
                    ));
                }
                Ok(())
            })
            .spawn()
    };

    let child = match child_result {
        Ok(c) => c,
        Err(e) => {
            error!("Failed to spawn process: {:?}", e);
            return Err(anyhow::anyhow!("Failed to spawn process: {:?}", e));
        }
    };

    let output = match child.wait_with_output() {
        Ok(o) => o,
        Err(e) => {
            error!("Failed to wait for child process: {:?}", e);
            return Err(anyhow::anyhow!("Failed to wait for process: {:?}", e));
        }
    };

    if !output.status.success() {
        return Err(anyhow::anyhow!(
            "Build failed with status: {}",
            output.status
        ));
    }

    Ok(())
}
