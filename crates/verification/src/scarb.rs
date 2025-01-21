use anyhow::Result;
use cairo_lang_starknet_classes::contract_class::ContractClass;
use scarb_api::ScarbCommand;
use std::fs::File;
use std::io::BufReader;
use std::path::PathBuf;
use std::{env, fs};
use semver::Version;
use tracing::error;
use walnut_shared::tuple_to_version_string;

use crate::artifacts::read_scarb_artifacts;
use crate::manifest::Manifest;
use crate::sozo::run_sozo_build;
use crate::ClassVerificationData;

fn supported_old_cairo_versions() -> Vec<Version> {
    vec![
        Version::parse("2.6.3").unwrap(),
        Version::parse("2.6.4").unwrap(),
        Version::parse("2.7.0").unwrap(),
    ]
}

fn minimum_supported_new_cario_version() -> Version {
    Version::parse("2.8.2").unwrap()
}
const SUPPORTED_DOJO_VERSIONS: &[&str] = &["v1.0.1", "v1.0.12"];

const BUILD_PROFILE: &str = "release";

fn run_scarb_build(tmp_dir: &PathBuf, scarb_path: &str) -> Result<()> {
    let mut cmd = ScarbCommand::new();
    cmd.current_dir(tmp_dir);
    let absolute_path = match fs::canonicalize(scarb_path) {
        Ok(path) => path,
        Err(e) => {
            let error_message = format!(
                "Failed to canonicalize scarb path: {:?}. Scarb path: {:?}",
                e, scarb_path
            );
            error!(error_message);
            return Err(anyhow::anyhow!(error_message));
        }
    };
    cmd.scarb_path(absolute_path);
    cmd.arg("--profile").arg(BUILD_PROFILE);
    cmd.arg("build");
    let scarb_cache_dir = env::current_dir()?.join(".cache/scarb");
    let scarb_cache_dir_str = scarb_cache_dir.to_str().ok_or_else(|| {
        error!("Error converting cache directory to string");
        anyhow::anyhow!("Failed to convert cache dir to string")
    })?;
    cmd.env("SCARB_CACHE", scarb_cache_dir_str);
    let mut process_cmd = cmd.command();
    if process_cmd.status()?.success() {
        Ok(())
    } else {
        let output = match process_cmd.output() {
            Ok(output) => output,
            Err(e) => {
                error!("Failed to execute `scarb`, failed to get output: {:?}", e);
                return Err(anyhow::anyhow!("Failed to compile the contract class"));
            }
        };
        let error_message = format!(
            "Failed to compile the contract class; `scarb` exited with error: {:?}",
            output
        );
        error!("{}", error_message);
        Err(anyhow::anyhow!(error_message))
    }
}

pub fn compile_with_scarb(
    starknet_version: (u32, u32, u32),
    manifest: Manifest,
    tmp_dir: &PathBuf,
    class_verification_data: &mut ClassVerificationData,
) -> Result<()> {
    if !is_cairo_version_supported(starknet_version) {
        return Err(anyhow::anyhow!(
            "Unsupported Cairo version {}. Contact us if you need support for a different version: https://t.me/walnuthq",
            tuple_to_version_string(starknet_version)
        ));
    }

    let binaries_save_directory_path = std::env::var("BINARIES_SAVE_DIRECTORY_PATH").unwrap_or("".to_string());
    let scarb_path = format!(
        "{}/scarb/scarb_cairo_v_{}_{}_{}",
        binaries_save_directory_path,
        starknet_version.0, starknet_version.1, starknet_version.2
    );
    run_scarb_build(tmp_dir, &scarb_path)?;

    for (class_hash, class_result) in class_verification_data.iter_mut() {
        let (class_name, _, _, _, _, _) = match class_result.as_ref() {
            Ok(result) => result,
            Err(e) => {
                let error_message =
                    format!("Error in class result for hash {}: {:?}", class_hash, e);
                error!("{}", error_message);
                continue;
            }
        };

        let contract_class_path = tmp_dir.join("target").join(BUILD_PROFILE).join(format!(
            "{}_{}.contract_class.json",
            manifest.package_name, class_name
        ));

        let contract_class_file = match File::open(&contract_class_path) {
            Ok(file) => file,
            Err(e) => {
                let error_message = format!("Failed to open contract class file: {:?}", e);
                error!("{}", error_message);
                *class_result = Err(anyhow::anyhow!(error_message));
                continue;
            }
        };

        let contract_class_reader = BufReader::new(contract_class_file);
        let contract_class: ContractClass = match serde_json::from_reader(contract_class_reader) {
            Ok(class) => class,
            Err(e) => {
                let error_message = format!("Failed to deserialize contract class: {:?}", e);
                error!("{}", error_message);
                *class_result = Err(anyhow::anyhow!(error_message));
                continue;
            }
        };

        let cairo_debug_info_path: Option<PathBuf> = match starknet_version {
            version if is_old_cairo_version_supported(version) => {
                Some(tmp_dir.join("target").join(BUILD_PROFILE).join(format!(
                    "{}_{}.contract_class_debug.json",
                    manifest.package_name, class_name
                )))
            }
            _ => None,
        };

        if let Ok((
            _,
            _,
            _,
            ref mut existing_contract_class,
            ref mut existing_cairo_debug_info_path,
            _,
        )) = class_result
        {
            *existing_contract_class = Some(contract_class);
            *existing_cairo_debug_info_path = cairo_debug_info_path;
        }
    }

    Ok(())
}

/// Builds a project at a given path using Scarb for Cairo version 2.8.0 or newer.
///
/// # Returns
///
/// A `Result` containing a vector of tuples with `ContractClass` and a class hash (`String`)
/// if successful, or an error if the build fails.
pub fn build_with_scarb(
    manifest: Manifest,
    tmp_dir: &PathBuf,
) -> Result<Vec<(String, ContractClass)>> {
    if !is_new_cairo_version_supported(manifest.cairo_version) {
        return Err(anyhow::anyhow!(
            "Unsupported Cairo version {}. Contact us if you need support for a different version: https://t.me/walnuthq",
            tuple_to_version_string(manifest.cairo_version)
        ));
    }

    if let Some(dojo_version) = manifest.dojo_version {
        if !SUPPORTED_DOJO_VERSIONS.contains(&dojo_version.as_str()) {
            return Err(anyhow::anyhow!(
                "Unsupported Dojo version {}. Currently, we support versions {}. Contact us if you need support for a different version: https://t.me/walnuthq",
                dojo_version,
                SUPPORTED_DOJO_VERSIONS.join(", ")
            ));
        }
        let binaries_save_directory_path = std::env::var("BINARIES_SAVE_DIRECTORY_PATH").unwrap_or("".to_string());
        let sozo_path = format!("{binaries_save_directory_path}/sozo/sozo_{}", dojo_version);
        run_sozo_build(tmp_dir, &sozo_path)?;
    } else {
        let binaries_save_directory_path = std::env::var("BINARIES_SAVE_DIRECTORY_PATH").unwrap_or("".to_string());
        let scarb_path = format!(
            "{}/scarb/scarb_cairo_v{}.{}.{}",
            binaries_save_directory_path,
            manifest.cairo_version.0, manifest.cairo_version.1, manifest.cairo_version.2
        );
        run_scarb_build(tmp_dir, &scarb_path)?;
    }

    read_scarb_artifacts(tmp_dir, &manifest.package_name, BUILD_PROFILE)
}

pub fn is_cairo_version_supported(version: (u32, u32, u32)) -> bool {
    is_old_cairo_version_supported(version) || is_new_cairo_version_supported(version)
}

pub fn is_new_cairo_version_supported(version: (u32, u32, u32)) -> bool {
    let version_string = tuple_to_version_string(version);
    let version_supported = match Version::parse(version_string.as_str()) {
        Ok(version) => version >= minimum_supported_new_cario_version(),
        Err(_) => {
            error!("Invalid cairo version on support check: {}", version_string.as_str());
            false
        }
    };
    version_supported
}

pub fn is_old_cairo_version_supported(version: (u32, u32, u32)) -> bool {
    let version_string = tuple_to_version_string(version);
    let old_version_supported = match Version::parse(version_string.as_str()) {
        Ok(version) => supported_old_cairo_versions().contains(&version),
        Err(_) => {
            error!("Invalid cairo version on support check: {}", version_string.as_str());
            false
        }
    };
    old_version_supported
}

