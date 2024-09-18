use anyhow::Result;
use cairo_lang_starknet_classes::contract_class::ContractClass;
use std::fs::File;
use std::io::BufReader;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::{env, fs};
use tracing::error;
use walkdir::WalkDir;

use crate::utils::Manifest;

pub const SUPPORTED_DOJO_ALPHA_VERSIONS: &[u8] = &[11, 12];

fn run_sozo_build(
    tmp_dir: &PathBuf,
    sozo_path: &str,
    is_output_debug_info_flag: bool,
) -> Result<()> {
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

    if is_output_debug_info_flag {
        cmd.arg("--output-debug-info");
    }

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

pub fn compile_with_sozo(
    _starknet_version: (u32, u32, u32),
    manifest: Manifest,
    tmp_dir: &PathBuf,
    class_name: String,
) -> Result<(ContractClass, Option<PathBuf>)> {
    // Check if the manifest contains a dojo_alpha_version
    if let Some(dojo_alpha_version) = manifest.dojo_alpha_version {
        // If the dojo_alpha_version is supported, construct the sozo_path and run the build
        if SUPPORTED_DOJO_ALPHA_VERSIONS.contains(&dojo_alpha_version) {
            let sozo_path = format!("binaries/sozo/sozo-v1-0-0-alpha-{}", dojo_alpha_version);
            let is_output_debug_info_flag = dojo_alpha_version >= 12;
            run_sozo_build(tmp_dir, &sozo_path, is_output_debug_info_flag).map_err(|e| {
                let error_message = format!("Failed to build the Dojo project: {:?}", e);
                error!("{}", error_message);
                anyhow::anyhow!(error_message)
            })?;
        } else {
            // If the dojo_alpha_version is not supported, log an error and return an error
            let error_message = format!(
                "Unsupported Dojo version. We support Dojo versions: {}.",
                get_supported_dojo_versions()
            );
            error!(error_message);
            return Err(anyhow::anyhow!(error_message));
        }
    } else {
        // If no dojo_alpha_version is specified, use the latest supported version
        let sozo_path = format!(
            "binaries/sozo/sozo-v1-0-0-alpha-{}",
            SUPPORTED_DOJO_ALPHA_VERSIONS.last().unwrap()
        );
        run_sozo_build(tmp_dir, &sozo_path, true).map_err(|e| {
            let error_message = format!(
                "Failed to build the Dojo project: {:?}. We support Dojo versions: {}.",
                e,
                get_supported_dojo_versions()
            );
            error!("{}", error_message);
            anyhow::anyhow!(error_message)
        })?;
    };

    // Hotfix for the class name
    let mut _class_name = class_name.clone();
    if _class_name.starts_with('-') {
        _class_name.remove(0);
    }

    let namespace_name = manifest
        .dojo_namespace_name
        .as_deref()
        .unwrap_or(&manifest.package_name);
    let file_name = format!("{}-{}.json", namespace_name, _class_name);

    let contract_class_path = WalkDir::new(tmp_dir.join("target"))
        .into_iter()
        .filter_map(|e| e.ok())
        .find(|e| e.file_name().to_string_lossy() == file_name)
        .map(|e| e.into_path())
        .ok_or_else(|| {
            let error_message = format!(
                "Failed to find contract class file '{}' in target directory",
                file_name
            );
            error!("{}", error_message);
            anyhow::anyhow!(error_message)
        })?;

    let contract_class_file = File::open(&contract_class_path).map_err(|e| {
        error!("Failed to open contract class file: {:?}", e);
        e
    })?;
    let contract_class_reader = BufReader::new(contract_class_file);
    let contract_class: ContractClass =
        serde_json::from_reader(contract_class_reader).map_err(|e| {
            error!("Failed to deserialize contract class: {:?}", e);
            e
        })?;

    // Assume debug info file is in the same folder with a different name format
    let debug_file_name = format!("{}-{}.debug.json", namespace_name, _class_name);
    let cairo_debug_info_path = Some(contract_class_path.parent().unwrap().join(debug_file_name));

    Ok((contract_class, cairo_debug_info_path))
}

pub fn get_supported_dojo_versions() -> String {
    SUPPORTED_DOJO_ALPHA_VERSIONS
        .iter()
        .map(|&version| format!("v1.0.0-alpha.{}", version))
        .collect::<Vec<String>>()
        .join(", ")
}
