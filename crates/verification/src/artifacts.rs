use crate::utils::{deserialize_json, read_file};

use anyhow::Result;
use cairo_lang_starknet_classes::contract_class::ContractClass;
use serde::{Deserialize, Serialize};
use starknet_rust::core::types::contract::SierraClass;
use std::fs;
use std::path::{Path, PathBuf};
use tracing::{debug, error, warn};

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
    tmp_dir: &Path,
    package_name: &str,
    build_profile: &str,
) -> Result<Vec<(String, ContractClass)>> {
    read_starknet_artifacts(tmp_dir, package_name, build_profile, false)?
        .into_iter()
        .map(|(class_hash, contract_class, _)| Ok((class_hash, contract_class)))
        .collect()
}

pub fn read_old_cairo_version_artifacts(
    tmp_dir: &Path,
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
    tmp_dir: &Path,
    package_name: &str,
    build_profile: &str,
    include_debug_info: bool,
) -> Result<Vec<(String, ContractClass, Option<PathBuf>)>> {
    let target_dir = tmp_dir.join("target").join(build_profile);

    // Read every package's artifacts, not just the root package's: in a workspace the
    // contract may live in a member while the root package defines none (or others).
    debug!("Scanning for artifact files in: {}", target_dir.display());
    let artifact_paths = list_artifact_files(&target_dir, package_name).map_err(|e| {
        error!("Failed to read target directory: {:?}", e);
        anyhow::anyhow!("Failed to scan for artifact files: {:?}", e)
    })?;

    if artifact_paths.is_empty() {
        return Err(anyhow::anyhow!(
            "No artifact files found in {}",
            target_dir.display()
        ));
    }

    let mut all_classes = Vec::new();
    for path in artifact_paths {
        debug!("Reading artifact file: {}", path.display());
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

    if all_classes.is_empty() {
        return Err(anyhow::anyhow!("No contracts found in any artifact files"));
    }

    Ok(all_classes)
}

/// Lists `*.starknet_artifacts.json` files in `target_dir`: `package_name`'s own file first,
/// then the rest sorted by name. The order must be stable across builds, because verification
/// pairs each non-inline class with its inline counterpart by position.
fn list_artifact_files(target_dir: &Path, package_name: &str) -> std::io::Result<Vec<PathBuf>> {
    let specific_path = target_dir.join(format!("{}.starknet_artifacts.json", package_name));

    let mut other_paths = Vec::new();
    for entry in fs::read_dir(target_dir)?.flatten() {
        let path = entry.path();
        if path
            .file_name()
            .and_then(|n| n.to_str())
            .is_some_and(|n| n.ends_with(".starknet_artifacts.json"))
            && path != specific_path
        {
            other_paths.push(path);
        }
    }
    other_paths.sort();

    let mut artifact_paths = Vec::new();
    if specific_path.exists() {
        artifact_paths.push(specific_path);
    }
    artifact_paths.extend(other_paths);
    Ok(artifact_paths)
}

fn read_single_artifact_file(
    artifact_path: &PathBuf,
    tmp_dir: &Path,
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
                warn!(
                    "Failed to process contract artifact {}: {:?}",
                    contract_artifact.contract_name, e
                );
                // Continue to next contract
            }
        }
    }

    Ok(classes)
}

/// Find the compiled class hash for a specific contract name in the build artifacts.
/// Used by Voyager compiler to select the correct contract from multi-contract projects.
///
/// Workspace members may define contracts with the same name, so pass `contract_package` when
/// it is known. Without it, the first match wins (`package_name`'s own artifacts first).
pub fn find_class_hash_by_contract_name(
    tmp_dir: &Path,
    package_name: &str,
    build_profile: &str,
    contract_name: &str,
    contract_package: Option<&str>,
) -> Result<Option<String>> {
    let target_dir = tmp_dir.join("target").join(build_profile);

    let mut found: Option<String> = None;
    for (artifact_path, contract_artifact) in list_contracts(&target_dir, package_name)? {
        if contract_artifact.contract_name != contract_name
            || contract_package.is_some_and(|p| p != contract_artifact.package_name)
        {
            continue;
        }
        let Some(sierra_path) = &contract_artifact.artifacts.sierra else {
            continue;
        };
        if found.is_some() {
            warn!(
                "Contract '{}' is defined in several packages; using the first match",
                contract_name
            );
            break;
        }
        let class_hash = sierra_class_hash(&target_dir.join(sierra_path))?;
        debug!(
            "Found contract '{}' in {} with class hash {}",
            contract_name,
            artifact_path.display(),
            class_hash
        );
        found = Some(class_hash);
    }

    Ok(found)
}

/// Find the package defining the contract compiled to `class_hash` in the build artifacts.
pub fn find_package_by_class_hash(
    tmp_dir: &Path,
    package_name: &str,
    build_profile: &str,
    class_hash: &str,
) -> Result<Option<String>> {
    let target_dir = tmp_dir.join("target").join(build_profile);

    for (_, contract_artifact) in list_contracts(&target_dir, package_name)? {
        let Some(sierra_path) = &contract_artifact.artifacts.sierra else {
            continue;
        };
        match sierra_class_hash(&target_dir.join(sierra_path)) {
            Ok(hash) if hash == class_hash => return Ok(Some(contract_artifact.package_name)),
            Ok(_) => {}
            Err(e) => warn!("Failed to hash {}: {:?}", sierra_path, e),
        }
    }

    Ok(None)
}

/// Every contract listed in the artifact files of `target_dir`, in `list_artifact_files` order,
/// paired with the artifact file that lists it.
fn list_contracts(
    target_dir: &Path,
    package_name: &str,
) -> Result<Vec<(PathBuf, ContractArtifacts)>> {
    let mut contracts = Vec::new();
    for artifact_path in list_artifact_files(target_dir, package_name).unwrap_or_default() {
        let contents = read_file(&artifact_path)?;
        let starknet_artifacts: StarknetArtifacts =
            deserialize_json(&contents, "StarknetArtifacts")?;
        contracts.extend(
            starknet_artifacts
                .contracts
                .into_iter()
                .map(|contract| (artifact_path.clone(), contract)),
        );
    }
    Ok(contracts)
}

fn sierra_class_hash(sierra_path: &Path) -> Result<String> {
    let contents = read_file(&sierra_path.to_path_buf())?;
    let sierra_class: SierraClass = deserialize_json(&contents, "SierraClass")?;
    sierra_class
        .class_hash()
        .map(|hash| hash.to_fixed_hex_string())
        .map_err(|e| anyhow::anyhow!("Failed to compute class hash: {:?}", e))
}

fn process_contract_artifact(
    tmp_dir: &Path,
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
                            error!(
                                "No statements_code_locations in coverage info for {}",
                                sierra_path
                            );
                        }
                    } else {
                        error!(
                            "No cairo-coverage annotation in sierra_program_debug_info for {}",
                            sierra_path
                        );
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

#[cfg(test)]
mod tests {
    use super::*;

    const FIRST_SIERRA: &str = include_str!("../tests/fixtures/first.contract_class.json");
    const SECOND_SIERRA: &str = include_str!("../tests/fixtures/second.contract_class.json");
    const FIRST_HASH: &str = "0x0378452aea24f7d86ff1db53c4325164eae98b0974dadd8e20c79b29b4bb9ab1";
    const SECOND_HASH: &str = "0x0792fb3d92c3cdf1ea486ced754905961cce3023d17679893410b8d3a9d485fc";

    /// A fake `target/release` of a Scarb workspace build: each entry is a package and the
    /// `(contract_name, sierra)` pairs its `<package>.starknet_artifacts.json` lists.
    fn write_target(packages: &[(&str, &[(&str, &str)])]) -> PathBuf {
        let tmp_dir = std::env::temp_dir().join(format!("artifacts-test-{}", uuid::Uuid::new_v4()));
        let target_dir = tmp_dir.join("target").join("release");
        fs::create_dir_all(&target_dir).unwrap();

        for (package_name, contracts) in packages {
            let mut artifacts = Vec::new();
            for (contract_name, sierra) in *contracts {
                let sierra_file = format!("{}_{}.contract_class.json", package_name, contract_name);
                fs::write(target_dir.join(&sierra_file), sierra).unwrap();
                artifacts.push(ContractArtifacts {
                    id: contract_name.to_string(),
                    package_name: package_name.to_string(),
                    contract_name: contract_name.to_string(),
                    module_path: format!("{}::{}", package_name, contract_name),
                    artifacts: ContractArtifact {
                        sierra: Some(sierra_file),
                        casm: None,
                    },
                });
            }
            let file = StarknetArtifacts {
                version: 1,
                contracts: artifacts,
            };
            fs::write(
                target_dir.join(format!("{}.starknet_artifacts.json", package_name)),
                serde_json::to_string(&file).unwrap(),
            )
            .unwrap();
        }

        tmp_dir
    }

    fn class_hashes(tmp_dir: &Path, package_name: &str) -> Vec<String> {
        read_new_cairo_version_artifacts(tmp_dir, package_name, "release")
            .unwrap()
            .into_iter()
            .map(|(class_hash, _)| class_hash)
            .collect()
    }

    #[test]
    fn test_root_package_without_contracts_reads_member_artifacts() {
        // e.g. OpenZeppelin's cairo-contracts: the root package only re-exports its members
        let tmp_dir = write_target(&[("root", &[]), ("member", &[("First", FIRST_SIERRA)])]);

        assert_eq!(class_hashes(&tmp_dir, "root"), vec![FIRST_HASH]);
        assert_eq!(
            find_class_hash_by_contract_name(&tmp_dir, "root", "release", "First", None).unwrap(),
            Some(FIRST_HASH.to_string())
        );

        fs::remove_dir_all(tmp_dir).unwrap();
    }

    #[test]
    fn test_same_contract_name_in_two_packages_is_told_apart_by_package() {
        let tmp_dir = write_target(&[
            ("root", &[]),
            ("a_member", &[("Token", FIRST_SIERRA)]),
            ("b_member", &[("Token", SECOND_SIERRA)]),
        ]);

        let package = find_package_by_class_hash(&tmp_dir, "root", "release", SECOND_HASH).unwrap();
        assert_eq!(package.as_deref(), Some("b_member"));

        let find = |contract_package| {
            find_class_hash_by_contract_name(&tmp_dir, "root", "release", "Token", contract_package)
                .unwrap()
        };
        assert_eq!(find(package.as_deref()), Some(SECOND_HASH.to_string()));
        assert_eq!(find(None), Some(FIRST_HASH.to_string()));
        assert_eq!(find(Some("other")), None);

        fs::remove_dir_all(tmp_dir).unwrap();
    }

    #[test]
    fn test_reads_all_packages_with_own_package_first() {
        // "a_member" sorts before "root", but the manifest's own package must come first
        let tmp_dir = write_target(&[
            ("root", &[("Second", SECOND_SIERRA)]),
            ("a_member", &[("First", FIRST_SIERRA)]),
        ]);

        assert_eq!(
            class_hashes(&tmp_dir, "root"),
            vec![SECOND_HASH, FIRST_HASH]
        );

        fs::remove_dir_all(tmp_dir).unwrap();
    }

    #[test]
    fn test_no_contracts_in_any_package_is_an_error() {
        let tmp_dir = write_target(&[("root", &[]), ("member", &[])]);

        assert!(read_new_cairo_version_artifacts(&tmp_dir, "root", "release").is_err());

        fs::remove_dir_all(tmp_dir).unwrap();
    }
}
