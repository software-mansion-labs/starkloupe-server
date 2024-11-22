use anyhow::Result;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::{env, fs};
use tracing::error;

pub fn run_sozo_build(tmp_dir: &PathBuf, sozo_path: &str) -> Result<()> {
    let absolute_path = fs::canonicalize(sozo_path)?;

    let scarb_cache_dir = env::current_dir()?.join(".cache/scarb");
    let scarb_cache_dir_str = scarb_cache_dir.to_str().ok_or_else(|| {
        error!("Error converting cache directory to string");
        anyhow::anyhow!("Failed to convert cache dir to string")
    })?;

    let mut cmd = Command::new(absolute_path);

    cmd.env("SCARB_CACHE", scarb_cache_dir_str)
        .arg("build")
        .current_dir(tmp_dir)
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit());

    let status = cmd.status()?;

    if status.success() {
        Ok(())
    } else {
        let output = match cmd.output() {
            Ok(output) => output,
            Err(e) => {
                let error_message = format!(
                    "Failed to execute `sozo`, status: {:?}, failed to get output: {:?}",
                    status, e
                );
                error!("{}", error_message);
                return Err(anyhow::anyhow!(error_message));
            }
        };
        let error_message = format!(
            "`sozo` exited with error: {:?}, status: {:?}",
            output, status
        );
        error!("{}", error_message);
        Err(anyhow::anyhow!(error_message))
    }
}
