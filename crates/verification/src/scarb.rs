use crate::artifacts::{read_new_cairo_version_artifacts, read_old_cairo_version_artifacts};
use crate::manifest::Manifest;
use crate::utils::set_limits;

use anyhow::Result;
use cairo_lang_starknet_classes::contract_class::ContractClass;
use semver::Version;
use std::os::unix::process::CommandExt;
use std::path::PathBuf;
use std::sync::OnceLock;
use std::time::Duration;
use std::{env, fs};
use tokio::process::Command;
use tokio::sync::Semaphore;
use tracing::{debug, error, info, warn};
use walnut_shared::tuple_to_version_string;

/// Global semaphore to limit concurrent builds
/// Default is 2 parallel builds, configurable via BUILD_CONCURRENCY_LIMIT env var
fn build_semaphore() -> &'static Semaphore {
    static SEMAPHORE: OnceLock<Semaphore> = OnceLock::new();
    SEMAPHORE.get_or_init(|| {
        let limit = std::env::var("BUILD_CONCURRENCY_LIMIT")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(3);
        Semaphore::new(limit)
    })
}

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

async fn run_project_build_for_profile(tmp_dir: &PathBuf, path: &str, profile: &str) -> Result<()> {
    // Acquire semaphore permit to limit concurrent builds
    let _permit = build_semaphore()
        .acquire()
        .await
        .map_err(|_| anyhow::anyhow!("Build semaphore closed"))?;

    let cpu_limit: u64 = std::env::var("BUILD_CPU_LIMIT")
        .unwrap_or("300".to_string())
        .parse::<u64>()?;

    let build_timeout_secs: u64 = std::env::var("BUILD_TIMEOUT_SECS")
        .unwrap_or("180".to_string())
        .parse::<u64>()?;
    let build_timeout = Duration::from_secs(build_timeout_secs);

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

    info!(
        "Running build command: {} --profile {} build",
        absolute_path.display(),
        profile
    );

    let build_start = std::time::Instant::now();
    let mut child = cmd.spawn()?;

    match tokio::time::timeout(build_timeout, child.wait()).await {
        Ok(Ok(status)) => {
            let duration = build_start.elapsed();
            if status.success() {
                info!(
                    "Build completed successfully in {:.2}s",
                    duration.as_secs_f64()
                );
                Ok(())
            } else {
                debug!(
                    "Build failed after {:.2}s. Status: {:?}",
                    duration.as_secs_f64(),
                    status
                );
                Err(anyhow::anyhow!(
                    "Project build failed with status: {:?}",
                    status
                ))
            }
        }
        Ok(Err(e)) => Err(anyhow::anyhow!("Failed to wait for process: {:?}", e)),
        Err(_) => {
            warn!("Build timed out after {}s", build_timeout_secs);
            Err(anyhow::anyhow!(
                "Build timed out after {}s",
                build_timeout_secs
            ))
        }
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

    run_project_build_for_profile(tmp_dir, &scarb_path, profile).await?;

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
        run_project_build_for_profile(tmp_dir, &sozo_path, profile).await?;
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
        run_project_build_for_profile(tmp_dir, &scarb_path, profile).await?;
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
