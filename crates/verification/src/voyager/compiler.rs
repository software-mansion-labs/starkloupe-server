use super::types::{CompilerVersion, VoyagerSourceResponse};
use crate::artifacts::find_class_hash_by_contract_name;
use crate::manifest::Manifest;
use crate::scarb::{
    build_with_scarb_for_profile, build_with_scarb_for_profile_no_validation,
    is_new_cairo_version_supported,
};
use crate::utils::{
    create_files_from_map, move_failed_verification_to_failed_tmp, remove_walnut_debug_from_scarb,
};
use anyhow::Result;
use cairo_lang_starknet_classes::contract_class::ContractClass;
use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;
use tracing::{error, info};
use uuid::Uuid;

/// Result of compiling external source code from Voyager
#[derive(Debug)]
pub struct CompiledExternalClass {
    /// Original class hash from Voyager (used as cache key)
    pub original_class_hash: String,
    /// Class hash after compilation with debug info (inline strategy)
    pub inline_class_hash: String,
    /// Compiled Sierra contract class (contains debug info in sierra_program_debug_info)
    pub contract_class: ContractClass,
    /// Non-inline compiled Sierra contract class (has debug info, CASM matches original).
    /// Used for simple trace function calls where PCs must match the original execution.
    pub non_inline_contract_class: Option<ContractClass>,
    /// Original source code (cleaned, without walnut-debug profile)
    pub source_code: HashMap<String, String>,
}

/// Compile source code fetched from Voyager API
///
/// This function:
/// 1. Creates a temporary directory
/// 2. Writes source files
/// 3. Modifies Scarb.toml to add walnut-debug profile
/// 4. Compiles with scarb
/// 5. Extracts debug info from compiled artifacts
/// 6. Cleans up temporary directory
pub async fn compile_voyager_source(
    source_response: VoyagerSourceResponse,
) -> Result<CompiledExternalClass> {
    let original_class_hash = source_response.class_hash.clone();

    // Parse compiler version
    let compiler_version = CompilerVersion::parse(&source_response.compiler_version)?;
    let version_tuple = compiler_version.as_tuple();

    // Check if version is supported
    if !is_new_cairo_version_supported(version_tuple) {
        return Err(anyhow::anyhow!(
            "Unsupported Cairo version {} from Voyager. Minimum supported is 2.8.2",
            source_response.compiler_version
        ));
    }

    // Create temporary directory
    let verification_id = format!("voyager-{}", Uuid::new_v4());
    let tmp_dir = PathBuf::from("tmp/verification").join(&verification_id);
    let tmp_dir_clone = tmp_dir.clone();

    info!(
        "Compiling Voyager source for class {} (version {}) in {}",
        original_class_hash,
        source_response.compiler_version,
        tmp_dir.display()
    );

    let source_response_clone = source_response.clone();
    let verified_name = source_response.verified_name.clone();
    let compiler_version_str = source_response.compiler_version.clone();
    let verified_name_for_prepare = verified_name.clone();

    // 1. Prepare files in blocking task
    let (manifest, profile, prepared_source_code) = tokio::task::spawn_blocking(move || {
        fs::create_dir_all(&tmp_dir_clone).map_err(|e| {
            anyhow::anyhow!(
                "Failed to create temp directory {}: {}",
                tmp_dir_clone.display(),
                e
            )
        })?;

        let mut source_code = source_response_clone.source_code.clone();

        // Pin version ranges to exact compiler version (handles missing Scarb.lock)
        pin_dependency_versions(&mut source_code, &compiler_version_str);

        // Parse manifest (this modifies Scarb.toml to add debug flags and walnut-debug profile)
        let manifest = match Manifest::new_with_verified_name(
            &mut source_code,
            Some(version_tuple),
            Some(&verified_name_for_prepare),
        ) {
            Ok(m) => m,
            Err(e) => {
                return Err(anyhow::anyhow!("Failed to parse manifest: {}", e));
            }
        };

        // Write source files to temp directory
        if let Err(e) = create_files_from_map(&source_code, &tmp_dir_clone) {
            return Err(anyhow::anyhow!("Failed to write source files: {}", e));
        }

        // Find the walnut-debug profile (or inline strategy profile)
        let profile = manifest
            .profile_with_inline_strategy
            .keys()
            .next()
            .cloned()
            .unwrap_or_else(|| "walnut-debug".to_string());

        Ok::<_, anyhow::Error>((manifest, profile, source_code))
    })
    .await??;

    info!(
        "Inline profile selected: '{}', profiles_with_inline_strategy: {:?}",
        profile,
        manifest
            .profile_with_inline_strategy
            .keys()
            .collect::<Vec<_>>()
    );

    // 2. Build with scarb (Async)
    let compiled_classes = match build_with_scarb_for_profile(&manifest, &tmp_dir, &profile).await {
        Ok(classes) => classes,
        Err(e) => {
            let tmp_dir_clone = tmp_dir.clone();
            let _ = tokio::task::spawn_blocking(move || {
                if let Err(move_err) = move_failed_verification_to_failed_tmp(&tmp_dir_clone) {
                    error!("Failed to move verification to failed tmp: {:?}", move_err);
                }
            })
            .await;
            return Err(anyhow::anyhow!("Compilation failed: {}", e));
        }
    };

    // 3. Find the matching contract class from compiled output
    if compiled_classes.is_empty() {
        return Err(anyhow::anyhow!("No contracts found in compiled output"));
    }

    let (inline_class_hash, contract_class) = if compiled_classes.len() == 1 {
        compiled_classes.into_iter().next().unwrap()
    } else {
        // Multi-contract project: match by verified_name from artifacts
        let verified_name_clone = verified_name.clone();
        let tmp_dir_clone = tmp_dir.clone();
        let package_name = manifest.package_name.clone();
        let profile_clone = profile.clone();
        let target_class_hash = tokio::task::spawn_blocking(move || {
            find_class_hash_by_contract_name(
                &tmp_dir_clone,
                &package_name,
                &profile_clone,
                &verified_name_clone,
            )
        })
        .await??;

        match target_class_hash {
            Some(hash) => compiled_classes
                .into_iter()
                .find(|(h, _)| h == &hash)
                .ok_or_else(|| {
                    anyhow::anyhow!(
                        "Compiled class hash {} for contract '{}' not found in output",
                        hash,
                        verified_name
                    )
                })?,
            None => {
                tracing::warn!(
                    "Could not find contract '{}' in artifacts, using first compiled class",
                    verified_name
                );
                compiled_classes.into_iter().next().unwrap()
            }
        }
    };

    // 4. Build with dev and release profiles (non-inline) to find the one matching on-chain class.
    // The non-inline class has debug info but no inlining-strategy, so CASM matches original.
    // We try both profiles because we don't know which was used to deploy the original contract.
    let non_inline_contract_class = {
        let profiles_to_try: Vec<&str> = ["dev", "release"]
            .iter()
            .filter(|p| **p != profile.as_str())
            .copied()
            .collect();

        let mut matched_class: Option<ContractClass> = None;
        for non_inline_profile in &profiles_to_try {
            info!(
                "Building with '{}' profile to find non-inline class matching on-chain hash {}",
                non_inline_profile, original_class_hash
            );
            match build_with_scarb_for_profile_no_validation(
                &manifest,
                &tmp_dir,
                non_inline_profile,
            )
            .await
            {
                Ok(classes) if !classes.is_empty() => {
                    info!(
                        "'{}' profile produced {} classes with hashes: {:?}",
                        non_inline_profile,
                        classes.len(),
                        classes.iter().map(|(h, _)| h.as_str()).collect::<Vec<_>>()
                    );
                    // Check if any compiled class hash matches the original on-chain class hash
                    if let Some((_, contract_class)) = classes
                        .into_iter()
                        .find(|(hash, _)| hash == &original_class_hash)
                    {
                        info!(
                            "Non-inline class from '{}' profile matches on-chain hash {}. has debug_info={}",
                            non_inline_profile,
                            original_class_hash,
                            contract_class.sierra_program_debug_info.is_some()
                        );
                        matched_class = Some(contract_class);
                        break;
                    } else {
                        info!(
                            "'{}' profile: no class hash matched on-chain hash {}",
                            non_inline_profile, original_class_hash
                        );
                    }
                }
                Ok(_) => {
                    info!("'{}' profile produced no classes", non_inline_profile);
                }
                Err(e) => {
                    info!(
                        "'{}' profile build failed (non-critical): {}",
                        non_inline_profile, e
                    );
                }
            }
        }
        if matched_class.is_none() {
            info!(
                "No non-inline profile matched on-chain hash {} for simple trace",
                original_class_hash
            );
        }
        matched_class
    };

    // 5. Clean up temp directory (Blocking)
    let tmp_dir_cleanup = tmp_dir.clone();
    if let Err(e) = tokio::task::spawn_blocking(move || {
        // cleanup_tmp_dir(&tmp_dir_cleanup);
    })
    .await
    {
        error!("Failed to cleanup temp dir task: {:?}", e);
    }

    // Remove walnut-debug from source code before storing (to match original)
    let mut clean_source_code = source_response.source_code;
    remove_walnut_debug_from_scarb(&mut clean_source_code);

    info!(
        "Successfully compiled Voyager source for class {} -> inline hash {}",
        original_class_hash, inline_class_hash
    );

    Ok(CompiledExternalClass {
        original_class_hash,
        inline_class_hash,
        contract_class,
        non_inline_contract_class,
        source_code: clean_source_code,
    })
}

/// Clean up temporary directory
fn cleanup_tmp_dir(tmp_dir: &PathBuf) {
    if let Err(e) = fs::remove_dir_all(tmp_dir) {
        tracing::warn!(
            "Failed to clean up temp directory {}: {:?}",
            tmp_dir.display(),
            e
        );
    }
}

/// Pin version range dependencies to exact compiler version and remove
/// dev-dependencies/scripts that can't be resolved without a Scarb.lock.
fn pin_dependency_versions(source_code: &mut HashMap<String, String>, compiler_version: &str) {
    let scarb_toml = match source_code.get("Scarb.toml") {
        Some(contents) => contents.clone(),
        None => return,
    };

    let mut toml_value = match scarb_toml.parse::<toml::Value>() {
        Ok(v) => v,
        Err(_) => return,
    };

    let mut modified = false;

    // Pin [dependencies].starknet
    if let Some(starknet) = toml_value
        .get_mut("dependencies")
        .and_then(|d| d.as_table_mut())
        .and_then(|deps| deps.get_mut("starknet"))
    {
        if let Some(version_str) = starknet.as_str() {
            if is_version_range(version_str) {
                *starknet = toml::Value::String(format!("={}", compiler_version));
                modified = true;
            }
        }
    }

    // Pin [package].cairo-version
    if let Some(cairo_ver) = toml_value
        .get_mut("package")
        .and_then(|p| p.as_table_mut())
        .and_then(|pkg| pkg.get_mut("cairo-version"))
    {
        if let Some(ver_str) = cairo_ver.as_str() {
            if is_version_range(ver_str) {
                *cairo_ver = toml::Value::String(format!("={}", compiler_version));
                modified = true;
            }
        }
    }

    // Remove [dev-dependencies] - not needed for contract compilation and
    // can't be resolved without a Scarb.lock (e.g. snforge_std = ">=0.55.0")
    if let Some(table) = toml_value.as_table_mut() {
        if table.remove("dev-dependencies").is_some() {
            info!("Removed [dev-dependencies] from Scarb.toml (not needed for build)");
            modified = true;
        }
        if table.remove("scripts").is_some() {
            modified = true;
        }
    }

    if modified {
        if let Ok(updated) = toml::to_string(&toml_value) {
            source_code.insert("Scarb.toml".to_string(), updated);
            info!(
                "Pinned version ranges in Scarb.toml to exact version {}",
                compiler_version
            );
        }
    }
}

fn is_version_range(version_str: &str) -> bool {
    version_str.starts_with(">=")
        || version_str.starts_with("<=")
        || version_str.starts_with('>')
        || version_str.starts_with('<')
        || version_str.starts_with('^')
        || version_str.starts_with('~')
}
