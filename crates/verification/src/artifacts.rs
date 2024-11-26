use anyhow::Result;
use cairo_lang_starknet_classes::contract_class::ContractClass;
use serde::{Deserialize, Serialize};
use starknet::core::types::contract::SierraClass;
use std::fs;
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

pub fn read_scarb_artifacts(
    tmp_dir: &PathBuf,
    package_name: &str,
    build_profile: &str,
) -> Result<Vec<(String, ContractClass)>> {
    let artifacts_file_path = tmp_dir
        .join("target")
        .join(build_profile)
        .join(format!("{}.starknet_artifacts.json", package_name));

    let artifacts_contents = fs::read_to_string(&artifacts_file_path)?;
    let starknet_artifacts: StarknetArtifacts = serde_json::from_str(&artifacts_contents)?;

    let mut classes: Vec<(String, ContractClass)> = Vec::new();

    for contract_artifact in &starknet_artifacts.contracts {
        if let Some(sierra_path) = &contract_artifact.artifacts.sierra {
            let contract_sierra_path = tmp_dir.join("target").join(build_profile).join(sierra_path);

            let contract_class_file_contents = match fs::read_to_string(&contract_sierra_path) {
                Ok(contents) => contents,
                Err(e) => {
                    let error_message = format!("Failed to read contract class file: {:?}", e);
                    error!("{}", error_message);
                    return Err(anyhow::anyhow!(error_message));
                }
            };

            let contract_class_v1: SierraClass =
                match serde_json::from_str(&contract_class_file_contents) {
                    Ok(class) => class,
                    Err(e) => {
                        let error_message =
                            format!("Failed to deserialize contract class: {:?}", e);
                        error!("{}", error_message);
                        return Err(anyhow::anyhow!(error_message));
                    }
                };

            let class_hash = match contract_class_v1.class_hash() {
                Ok(hash) => hash.to_fixed_hex_string(),
                Err(e) => {
                    let error_message = format!("Failed to compute class hash: {:?}", e);
                    error!(error_message);
                    return Err(anyhow::anyhow!(error_message));
                }
            };

            let contract_class_v2: ContractClass =
                match serde_json::from_str(&contract_class_file_contents) {
                    Ok(class) => class,
                    Err(e) => {
                        let error_message =
                            format!("Failed to deserialize contract class: {:?}", e);
                        error!("{}", error_message);
                        return Err(anyhow::anyhow!(error_message));
                    }
                };

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

            classes.push((class_hash, contract_class_v2));
        }
    }

    Ok(classes)
}
