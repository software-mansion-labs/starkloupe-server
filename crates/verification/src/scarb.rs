use crate::artifacts::{read_new_cairo_version_artifacts, read_old_cairo_version_artifacts};
use crate::manifest::Manifest;
use crate::utils::set_limits;

use anyhow::Result;
use cairo_lang_starknet_classes::contract_class::ContractClass;
use semver::Version;
use std::os::unix::process::CommandExt;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::{env, fs};
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

fn run_project_build_for_profile(tmp_dir: &PathBuf, path: &str, profile: &str) -> Result<()> {
    // Default limit is 300s = 5min
    let cpu_limit: u64 = std::env::var("BUILD_CPU_LIMIT")
        .unwrap_or("300".to_string())
        .parse::<u64>()?;

    info!("Build project with {:?}", &path);
    let absolute_path = match fs::canonicalize(path) {
        Ok(path) => Ok(path),
        Err(e) => {
            let binary = path
                .split("binaries/")
                .nth(1)
                .unwrap_or("unknown component");
            error!("{}", e);
            Err(anyhow::anyhow!("Cannot find {} for verification", binary))
        }
    }?;

    let child_result = unsafe {
        Command::new(absolute_path)
            .current_dir(tmp_dir)
            .arg("--profile")
            .arg(profile)
            .arg("build")
            .stdout(Stdio::piped())
            .pre_exec(move || {
                if let Err(e) = set_limits(cpu_limit) {
                    error!("Failed to set resource limits: {:?}", e);
                    return Err(std::io::Error::new(
                        std::io::ErrorKind::Other,
                        format!("Failed to set resource limits: {}", e),
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
            return Err(anyhow::anyhow!("Failed to run process: {:?}", e));
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
        let stdout = String::from_utf8_lossy(&output.stdout);
        let stderr = String::from_utf8_lossy(&output.stderr);

        // For other errors, show the actual Scarb error
        error!(
            "Build failed. Status: {:?}, Stdout: {:?}, Stderr: {:?}",
            output.status, stdout, stderr
        );

        // Extract the actual error message from stderr (Scarb errors are usually in stderr)
        let error_message = if !stderr.is_empty() {
            stderr.to_string()
        } else {
            stdout.to_string()
        };

        // Return the actual Scarb error message to the user
        return Err(anyhow::anyhow!("Project build failed: {}", error_message));
    }

    Ok(())
}

/// Builds a project at a given path and with given profile using Scarb for Cairo version pre 2.8.0.
///
/// # Returns
///
/// A `Result` containing a vector of tuples with `ContractClass` and `Path`(to file that contain cairo locations) and a class hash (`String`)
/// if successful, or an error if the build fails.
pub fn compile_with_scarb_for_profile(
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

    run_project_build_for_profile(tmp_dir, &scarb_path, profile)?;

    read_old_cairo_version_artifacts(tmp_dir, &manifest.package_name, profile)
}

/// Builds a project at a given path and with given profile using Scarb for Cairo version 2.8.0 or newer.
///
/// # Returns
///
/// A `Result` containing a vector of tuples with `ContractClass` and a class hash (`String`)
/// if successful, or an error if the build fails.
pub fn build_with_scarb_for_profile(
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
        run_project_build_for_profile(tmp_dir, &sozo_path, profile)?;
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
        run_project_build_for_profile(tmp_dir, &scarb_path, profile)?;
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
