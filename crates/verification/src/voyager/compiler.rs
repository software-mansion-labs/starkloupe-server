use super::types::{CompilerVersion, VoyagerSourceResponse};
use crate::manifest::Manifest;
use crate::scarb::{build_with_scarb_for_profile, is_new_cairo_version_supported};
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
    fs::create_dir_all(&tmp_dir)?;

    info!(
        "Compiling Voyager source for class {} (version {}) in {}",
        original_class_hash,
        source_response.compiler_version,
        tmp_dir.display()
    );

    // Clone source code so we can modify it
    let mut source_code = source_response.source_code.clone();

    // Parse manifest (this modifies Scarb.toml to add debug flags and walnut-debug profile)
    // Use verified_name from Voyager as fallback if package name not in Scarb.toml
    let manifest = match Manifest::new_with_verified_name(
        &mut source_code,
        Some(version_tuple),
        Some(&source_response.verified_name),
    ) {
        Ok(m) => m,
        Err(e) => {
            cleanup_tmp_dir(&tmp_dir);
            return Err(anyhow::anyhow!("Failed to parse manifest: {}", e));
        }
    };

    // Write source files to temp directory
    if let Err(e) = create_files_from_map(&source_code, &tmp_dir) {
        cleanup_tmp_dir(&tmp_dir);
        return Err(anyhow::anyhow!("Failed to write source files: {}", e));
    }

    // Find the walnut-debug profile (or inline strategy profile)
    let profile = manifest
        .profile_with_inline_strategy
        .keys()
        .next()
        .cloned()
        .unwrap_or_else(|| "walnut-debug".to_string());

    info!("Building with profile: {}", profile);

    // Build with scarb
    let compiled_classes = match build_with_scarb_for_profile(&manifest, &tmp_dir, &profile) {
        Ok(classes) => classes,
        Err(e) => {
            error!("Failed to compile Voyager source: {:?}", e);
            if let Err(move_err) = move_failed_verification_to_failed_tmp(&tmp_dir) {
                error!("Failed to move verification to failed tmp: {:?}", move_err);
            }
            return Err(anyhow::anyhow!("Compilation failed: {}", e));
        }
    };

    // Clean up temp directory
    cleanup_tmp_dir(&tmp_dir);

    // Find the matching contract class
    // The compiled classes will have different hashes due to debug info
    if compiled_classes.is_empty() {
        return Err(anyhow::anyhow!("No contracts found in compiled output"));
    }

    // For now, take the first compiled class
    // In a multi-contract project, we might need smarter matching
    let (inline_class_hash, contract_class) = compiled_classes.into_iter().next().unwrap();

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
