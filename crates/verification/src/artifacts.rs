use crate::utils::{deserialize_json, read_file};

use anyhow::Result;
use cairo_lang_starknet_classes::contract_class::ContractClass;
use serde::{Deserialize, Serialize};
use starknet_rust::core::types::contract::SierraClass;
use std::path::PathBuf;
use tracing::error;

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
    let artifacts_file_path = tmp_dir
        .join("target")
        .join(build_profile)
        .join(format!("{}.starknet_artifacts.json", package_name));

    let artifacts_contents = read_file(&artifacts_file_path)?;
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
                error!("Failed to process contract artifact: {:?}", e);
                return Err(e);
            }
        }
    }

    Ok(classes)
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
                            error!("No statements code locations found in coverage info");
                            return Err(anyhow::anyhow!(
                                "No statements code locations found in coverage info"
                            ));
                        }
                    } else {
                        error!("No coverage info found in contract class");
                        return Err(anyhow::anyhow!("No coverage info found in contract class"));
                    }
                }
                None => {
                    let error_message = "No debug info found in contract class";
                    error!("{}", error_message);
                    return Err(anyhow::anyhow!(error_message));
                }
            };
            None
        };

        Ok((class_hash, contract_class_v2, debug_info_path))
    } else {
        Err(anyhow::anyhow!("Missing Sierra path in contract artifact"))
    }
}
