use crate::utils::{deserialize_json, read_file};

use anyhow::Result;
use cairo_lang_starknet_classes::contract_class::ContractClass;
use serde::{Deserialize, Serialize};
use starknet_rust::core::types::contract::SierraClass;
use std::fs;
use std::path::PathBuf;
use tracing::{error, info, warn};

#[derive(Debug, Serialize, Deserialize)]
struct StarknetArtifacts {
    version: usize,
    contracts: Vec<ContractArtifacts>,
}

#[derive(Debug, Serialize, Deserialize)]
struct ContractArtifacts {
    id: String,
    package_name: String, // Assuming PackageName is a type alias for String
    contract_name: String,
    module_path: String,
    artifacts: ContractArtifact,
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct ContractArtifact {
    sierra: Option<String>,
    casm: Option<String>,
}

pub fn read_new_cairo_version_artifacts(
    tmp_dir: &PathBuf,
    package_name: &str,
    build_profile: &str,
) -> Result<Vec<(String, ContractClass)>> {
    read_starknet_artifacts(tmp_dir, package_name, build_profile, false)?
        .into_iter()
        .map(|(class_hash, contract_class, _)| Ok((class_hash, contract_class)))
        .collect()
}

pub fn read_old_cairo_version_artifacts(
    tmp_dir: &PathBuf,
    package_name: &str,
    build_profile: &str,
) -> Result<Vec<(String, ContractClass, PathBuf)>> {
    read_starknet_artifacts(tmp_dir, package_name, build_profile, true)?
        .into_iter()
        .map(|(class_hash, contract_class, debug_info_path)| {
            let debug_info_path =
                debug_info_path.ok_or_else(|| anyhow::anyhow!("Debug info path missing"))?;
            Ok((class_hash, contract_class, debug_info_path))
        })
        .collect()
}

pub fn read_starknet_artifacts(
    tmp_dir: &PathBuf,
    package_name: &str,
    build_profile: &str,
    include_debug_info: bool,
) -> Result<Vec<(String, ContractClass, Option<PathBuf>)>> {
    let target_dir = tmp_dir.join("target").join(build_profile);

    // First try the specific package artifact file
    let specific_artifact_path = target_dir.join(format!("{}.starknet_artifacts.json", package_name));

    if specific_artifact_path.exists() {
        info!("Reading artifact file: {}", specific_artifact_path.display());
        return read_single_artifact_file(&specific_artifact_path, tmp_dir, build_profile, include_debug_info);
    }

    // If not found, scan for all *.starknet_artifacts.json files (workspace case)
    info!("Scanning for artifact files in: {}", target_dir.display());

    let mut all_classes = Vec::new();
    let mut found_any = false;

    match fs::read_dir(&target_dir) {
        Ok(entries) => {
            for entry in entries.flatten() {
                let path = entry.path();
                if path.extension().map_or(false, |ext| ext == "json")
                    && path
                        .file_name()
                        .and_then(|n| n.to_str())
                        .map_or(false, |n| n.ends_with(".starknet_artifacts.json"))
                {
                    found_any = true;
                    info!("Reading artifact file: {}", path.display());
                    match read_single_artifact_file(&path, tmp_dir, build_profile, include_debug_info) {
                        Ok(classes) => {
                            all_classes.extend(classes);
                        }
                        Err(e) => {
                            warn!("Failed to read artifact file {}: {:?}", path.display(), e);
                            // Continue to next artifact file
                        }
                    }
                }
            }
        }
        Err(e) => {
            error!("Failed to read target directory: {:?}", e);
            return Err(anyhow::anyhow!("Failed to scan for artifact files: {:?}", e));
        }
    }

    if !found_any {
        return Err(anyhow::anyhow!(
            "No artifact files found in {}",
            target_dir.display()
        ));
    }

    if all_classes.is_empty() {
        return Err(anyhow::anyhow!("No contracts found in any artifact files"));
    }

    Ok(all_classes)
}

fn read_single_artifact_file(
    artifact_path: &PathBuf,
    tmp_dir: &PathBuf,
    build_profile: &str,
    include_debug_info: bool,
) -> Result<Vec<(String, ContractClass, Option<PathBuf>)>> {
    let artifacts_contents = read_file(artifact_path)?;
    let starknet_artifacts: StarknetArtifacts =
        deserialize_json(&artifacts_contents, "StarknetArtifacts")?;

    let mut classes = Vec::new();
    for contract_artifact in &starknet_artifacts.contracts {
        match process_contract_artifact(
            tmp_dir,
            build_profile,
            contract_artifact,
            include_debug_info,
        ) {
            Ok((class_hash, contract_class, debug_info_path)) => {
                classes.push((class_hash, contract_class, debug_info_path));
            }
            Err(e) => {
                warn!("Failed to process contract artifact {}: {:?}", contract_artifact.contract_name, e);
                // Continue to next contract
            }
        }
    }

    Ok(classes)
}

/// Find the compiled class hash for a specific contract name in the build artifacts.
/// Used by Voyager compiler to select the correct contract from multi-contract projects.
pub fn find_class_hash_by_contract_name(
    tmp_dir: &PathBuf,
    package_name: &str,
    build_profile: &str,
    contract_name: &str,
) -> Result<Option<String>> {
    let target_dir = tmp_dir.join("target").join(build_profile);

    // Collect all artifact files to search: specific package first, then all others
    let mut artifact_paths = Vec::new();
    let specific_path = target_dir.join(format!("{}.starknet_artifacts.json", package_name));
    if specific_path.exists() {
        artifact_paths.push(specific_path.clone());
    }

    // Also scan all artifact files (for workspace projects)
    if let Ok(entries) = fs::read_dir(&target_dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path
                .file_name()
                .and_then(|n| n.to_str())
                .map_or(false, |n| n.ends_with(".starknet_artifacts.json"))
                && path != specific_path
            {
                artifact_paths.push(path);
            }
        }
    }

    for artifact_path in artifact_paths {
        let contents = read_file(&artifact_path)?;
        let starknet_artifacts: StarknetArtifacts =
            deserialize_json(&contents, "StarknetArtifacts")?;

        for contract_artifact in &starknet_artifacts.contracts {
            if contract_artifact.contract_name == contract_name {
                if let Some(sierra_path) = &contract_artifact.artifacts.sierra {
                    let contract_sierra_path = target_dir.join(sierra_path);
                    let contract_class_contents = read_file(&contract_sierra_path)?;
                    let contract_class_v1: SierraClass =
                        deserialize_json(&contract_class_contents, "SierraClass")?;
                    let class_hash = contract_class_v1
                        .class_hash()
                        .map(|hash| hash.to_fixed_hex_string())
                        .map_err(|e| {
                            anyhow::anyhow!("Failed to compute class hash: {:?}", e)
                        })?;
                    info!(
                        "Found contract '{}' in {} with class hash {}",
                        contract_name,
                        artifact_path.display(),
                        class_hash
                    );
                    return Ok(Some(class_hash));
                }
            }
        }
    }

    Ok(None)
}


fn process_contract_artifact(
    tmp_dir: &PathBuf,
    build_profile: &str,
    contract_artifact: &ContractArtifacts,
    include_debug_info_file: bool,
) -> Result<(String, ContractClass, Option<PathBuf>)> {
    if let Some(sierra_path) = &contract_artifact.artifacts.sierra {
        let contract_sierra_path = tmp_dir.join("target").join(build_profile).join(sierra_path);
        let contract_class_file_contents = read_file(&contract_sierra_path)?;

        let contract_class_v1: SierraClass =
            deserialize_json(&contract_class_file_contents, "SierraClass")?;
        let class_hash = contract_class_v1
            .class_hash()
            .map(|hash| hash.to_fixed_hex_string())
            .map_err(|e| {
                let error_message = format!("Failed to compute class hash: {:?}", e);
                error!("{}", error_message);
                anyhow::anyhow!(error_message)
            })?;

        let contract_class_v2: ContractClass =
            deserialize_json(&contract_class_file_contents, "ContractClass")?;

        let debug_info_path = if include_debug_info_file {
            Some(tmp_dir.join("target").join(build_profile).join(format!(
                "{}_{}.contract_class_debug.json",
                contract_artifact.package_name, contract_artifact.contract_name
            )))
        } else {
            match &contract_class_v2.sierra_program_debug_info {
                Some(debug_info) => {
                    if let Some(coverage_info) = debug_info
                        .annotations
                        .get("github.com/software-mansion/cairo-coverage")
                    {
                        if !coverage_info
                            .as_object()
                            .unwrap()
                            .contains_key("statements_code_locations")
                        {
                            error!("No statements_code_locations in coverage info for {}", sierra_path);
                        }
                    } else {
                        error!("No cairo-coverage annotation in sierra_program_debug_info for {}", sierra_path);
                    }
                }
                None => {
                    error!("No sierra_program_debug_info for {}", sierra_path);
                }
            };
            None
        };

        Ok((class_hash, contract_class_v2, debug_info_path))
    } else {
        Err(anyhow::anyhow!("Missing Sierra path in contract artifact"))
    }
}
