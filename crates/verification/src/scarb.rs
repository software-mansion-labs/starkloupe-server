use crate::artifacts::{read_new_cairo_version_artifacts, read_old_cairo_version_artifacts};
use crate::manifest::Manifest;
use crate::utils::set_limits;

use anyhow::Result;
use cairo_lang_starknet_classes::contract_class::ContractClass;
use semver::Version;
use std::path::PathBuf;
use std::time::Duration;
use std::{env, fs};
use tokio::process::Command;
use tracing::{error, info};
use walnut_shared::tuple_to_version_string;

fn supported_old_cairo_versions() -> Vec<Version> {
    vec![
        Version::parse("2.6.3").unwrap(),
        Version::parse("2.6.4").unwrap(),
        Version::parse("2.7.0").unwrap(),
    ]
}

fn supported_old_dojo_versions() -> Vec<Version> {
    vec![
        Version::parse("1.0.1").unwrap(),
        Version::parse("1.0.12").unwrap(),
    ]
}

fn minimum_supported_new_cairo_version() -> Version {
    Version::parse("2.8.2").unwrap()
}

fn minimum_supported_new_dojo_version() -> Version {
    Version::parse("1.1.0").unwrap()
}

/// Returns the default build timeout read from `BUILD_TIMEOUT_SECS` env var (default 120s).
/// Pass the result to `build_with_scarb_for_profile` / `compile_with_scarb_for_profile`.
/// Use `None` to skip the tokio timeout (background retries rely on OS CPU limit only).
pub fn default_build_timeout() -> Option<Duration> {
    let secs: u64 = std::env::var("BUILD_TIMEOUT_SECS")
        .unwrap_or("120".to_string())
        .parse()
        .unwrap_or(120);
    Some(Duration::from_secs(secs))
}

/// True if this error is the tokio timeout raised by `run_project_build_for_profile`.
/// Centralized so callers don't string-match on the message.
pub fn is_build_timeout_error(e: &anyhow::Error) -> bool {
    e.to_string().contains("Build timed out after")
}

/// `build_timeout`: `Some(d)` wraps with `tokio::time::timeout`; `None` waits without a deadline
/// (the OS-level CPU limit from `BUILD_CPU_LIMIT` still applies).
async fn run_project_build_for_profile(
    tmp_dir: &PathBuf,
    path: &str,
    profile: &str,
    build_timeout: Option<Duration>,
) -> Result<()> {
    let cpu_limit: u64 = std::env::var("BUILD_CPU_LIMIT")
        .unwrap_or("300".to_string())
        .parse::<u64>()?;

    let absolute_path = fs::canonicalize(path).map_err(|e| {
        let binary = path
            .split("binaries/")
            .nth(1)
            .unwrap_or("unknown component");
        error!("{}", e);
        anyhow::anyhow!("Cannot find {} for verification", binary)
    })?;

    let mut cmd = Command::new(&absolute_path);
    cmd.current_dir(tmp_dir)
        .arg("--profile")
        .arg(profile)
        .arg("build")
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .kill_on_drop(true);

    unsafe {
        cmd.pre_exec(move || {
            set_limits(cpu_limit).map_err(|e| {
                std::io::Error::new(
                    std::io::ErrorKind::Other,
                    format!("Failed to set resource limits: {}", e),
                )
            })
        });
    }

    let build_start = std::time::Instant::now();
    let mut child = cmd.spawn()?;

    let wait_result = if let Some(timeout_duration) = build_timeout {
        match tokio::time::timeout(timeout_duration, child.wait_with_output()).await {
            Ok(inner) => inner,
            Err(_) => {
                return Err(anyhow::anyhow!(
                    "Build timed out after {}s",
                    timeout_duration.as_secs()
                ));
            }
        }
    } else {
        child.wait_with_output().await
    };

    match wait_result {
        Ok(output) => {
            let duration = build_start.elapsed();
            if output.status.success() {
                info!(
                    "Build completed successfully in {:.2}s with command: {:?} --profile {} build on {:?} ",
                    duration.as_secs_f64(),
                    absolute_path.display(),
                    profile,
                    tmp_dir
                );
                Ok(())
            } else {
                let stdout = String::from_utf8_lossy(&output.stdout);
                let stderr = String::from_utf8_lossy(&output.stderr);
                let combined = format!("{}{}", stdout, stderr);
                error!(
                    "Build failed after {:.2}s with command: {:?} --profile {} build on {:?}. Status: {:?}\nOutput: {}",
                    duration.as_secs_f64(),
                    absolute_path.display(),
                    profile,
                    tmp_dir,
                    output.status,
                    combined
                );
                Err(anyhow::anyhow!("Project build failed: {}", combined))
            }
        }
        Err(e) => Err(anyhow::anyhow!("Failed to wait for process: {:?}", e)),
    }
}

/// Builds a project at a given path and with given profile using Scarb for Cairo version pre 2.8.0.
///
/// # Returns
///
/// A `Result` containing a vector of tuples with `ContractClass` and `Path`(to file that contain cairo locations) and a class hash (`String`)
/// if successful, or an error if the build fails.
pub async fn compile_with_scarb_for_profile(
    manifest: &Manifest,
    starknet_version: (u32, u32, u32),
    tmp_dir: &PathBuf,
    profile: &str,
    build_timeout: Option<Duration>,
) -> Result<Vec<(String, ContractClass, PathBuf)>> {
    if !is_cairo_version_supported(starknet_version) {
        return Err(anyhow::anyhow!(
            "Unsupported Cairo version {}. Contact us if you need support for a different version: https://t.me/walnuthq",
            tuple_to_version_string(starknet_version)
        ));
    }

    let binaries_save_directory_path =
        std::env::var("BINARIES_SAVE_DIRECTORY_PATH").unwrap_or("".to_string());
    let scarb_path = format!(
        "{}/scarb/scarb_cairo_v_{}_{}_{}",
        binaries_save_directory_path, starknet_version.0, starknet_version.1, starknet_version.2
    );

    run_project_build_for_profile(tmp_dir, &scarb_path, profile, build_timeout).await?;

    read_old_cairo_version_artifacts(tmp_dir, &manifest.package_name, profile)
}

/// Builds a project at a given path and with given profile using Scarb for Cairo version 2.8.0 or newer.
///
/// # Returns
///
/// A `Result` containing a vector of tuples with `ContractClass` and a class hash (`String`)
/// if successful, or an error if the build fails.
pub async fn build_with_scarb_for_profile(
    manifest: &Manifest,
    tmp_dir: &PathBuf,
    profile: &str,
    build_timeout: Option<Duration>,
) -> Result<Vec<(String, ContractClass)>> {
    if !is_new_cairo_version_supported(manifest.cairo_version) {
        error!(
            "Unsupported cairo version {}",
            tuple_to_version_string(manifest.cairo_version)
        );
        return Err(anyhow::anyhow!(
            "Unsupported Cairo version {}. Contact us if you need support for a different version: https://t.me/walnuthq",
            tuple_to_version_string(manifest.cairo_version)
        ));
    }

    if let Some(dojo_version) = &manifest.dojo_version {
        if !is_dojo_version_supported(dojo_version) {
            return Err(anyhow::anyhow!(
                "Unsupported Dojo version {}.",
                dojo_version
            ));
        }
        let binaries_save_directory_path =
            env::var("BINARIES_SAVE_DIRECTORY_PATH").unwrap_or("".to_string());
        let sozo_path = format!("{binaries_save_directory_path}/sozo/sozo_{}", dojo_version);
        run_project_build_for_profile(tmp_dir, &sozo_path, profile, build_timeout).await?;
    } else {
        let binaries_save_directory_path =
            env::var("BINARIES_SAVE_DIRECTORY_PATH").unwrap_or("".to_string());
        let scarb_path = format!(
            "{}/scarb/scarb_cairo_v{}.{}.{}",
            binaries_save_directory_path,
            manifest.cairo_version.0,
            manifest.cairo_version.1,
            manifest.cairo_version.2
        );
        run_project_build_for_profile(tmp_dir, &scarb_path, profile, build_timeout).await?;
    }
    read_new_cairo_version_artifacts(tmp_dir, &manifest.package_name, profile)
}

pub fn is_cairo_version_supported(version: (u32, u32, u32)) -> bool {
    is_old_cairo_version_supported(version) || is_new_cairo_version_supported(version)
}

pub fn is_old_cairo_version_supported(version: (u32, u32, u32)) -> bool {
    is_old_version_supported(
        tuple_to_version_string(version).as_str(),
        &supported_old_cairo_versions(),
        "cairo",
    )
}

pub fn is_new_cairo_version_supported(version: (u32, u32, u32)) -> bool {
    is_new_version_supported(
        tuple_to_version_string(version).as_str(),
        minimum_supported_new_cairo_version(),
        "cairo",
    )
}

pub fn is_dojo_version_supported(version: &str) -> bool {
    is_old_dojo_version_supported(version) || is_new_dojo_version_supported(version)
}

pub fn is_old_dojo_version_supported(version: &str) -> bool {
    is_old_version_supported(version, &supported_old_dojo_versions(), "dojo")
}

pub fn is_new_dojo_version_supported(version: &str) -> bool {
    is_new_version_supported(version, minimum_supported_new_dojo_version(), "dojo")
}

fn is_old_version_supported(
    version: &str,
    supported_old_versions: &[Version],
    tool_name: &str,
) -> bool {
    let version_stripped = version.strip_prefix('v').unwrap_or(version);
    match Version::parse(version_stripped) {
        Ok(ver) => supported_old_versions.contains(&ver),
        Err(_) => {
            error!(
                "Invalid {} version on support check: {}",
                tool_name, version
            );
            false
        }
    }
}

fn is_new_version_supported(
    version: &str,
    minimum_supported_version: Version,
    tool_name: &str,
) -> bool {
    let version_stripped = version.strip_prefix('v').unwrap_or(version);
    match Version::parse(version_stripped) {
        Ok(ver) => ver >= minimum_supported_version,
        Err(_) => {
            error!(
                "Invalid {} version on support check: {}",
                tool_name, version
            );
            false
        }
    }
}
