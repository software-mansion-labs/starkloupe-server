use super::types::{CompilerVersion, VoyagerSourceResponse};
use crate::artifacts::find_class_hash_by_contract_name;
use crate::manifest::Manifest;
use crate::scarb::{build_with_scarb_for_profile, is_new_cairo_version_supported};
use crate::utils::{
    create_files_from_map, move_failed_verification_to_failed_tmp, remove_walnut_debug_from_scarb,
};
use anyhow::Result;
use cairo_lang_starknet_classes::contract_class::ContractClass;
use scarb_dep_resolver::{is_dep_resolution_error, resolve_registry_deps};
use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::Duration;
use tracing::{debug, error, info, warn};
use uuid::Uuid;

/// Result of compiling external source code from Voyager
#[derive(Debug)]
pub struct CompiledExternalClass {
    /// Original class hash from Voyager (used as cache key)
    pub original_class_hash: String,
    /// Class hash after compilation with inline avoid strategy
    pub inline_class_hash: String,
    /// Compiled Sierra contract class with inline avoid strategy (has coverage annotations)
    pub contract_class: ContractClass,
    /// Non-inline compiled Sierra contract class (CASM matches original on-chain).
    /// Used for simple trace function calls where PCs must match the original execution.
    pub original_contract_class: Option<ContractClass>,
    /// Original source code (cleaned, without walnut-debug profile)
    pub source_code: HashMap<String, String>,
    /// Cairo version parsed from the manifest (used when persisting verification metadata)
    pub cairo_version: (u32, u32, u32),
    /// Package name from the manifest (used when persisting verification metadata)
    pub package_name: String,
    /// Profile name that produced the inline class (recorded in class_hash_profiles)
    pub inline_profile: String,
    /// Profile name that produced `original_contract_class` (e.g. "release" or "dev").
    /// None when no non-inline profile matched the on-chain hash.
    pub original_profile: Option<String>,
}

/// Result of Phase 1 (non-inline) compilation.
///
/// Phase 1 builds release (and optionally dev) profiles to find the class
/// whose CASM matches the original on-chain class.
///
/// If the matching profile already had inline avoid strategy, `inline_already_built`
/// is populated and Phase 2 is not needed (both original and inline class are available).
#[derive(Debug)]
pub struct Phase1Result {
    pub original_class_hash: String,
    /// Non-inline class whose CASM matches the on-chain original. None if no profile matched.
    pub original_contract_class: Option<ContractClass>,
    /// Name of the non-inline profile that produced `original_contract_class`
    /// (e.g. "release" or "dev"). None when no profile matched.
    pub original_profile: Option<String>,
    /// Set when the matching profile already had inline avoid strategy —
    /// contains (inline_class_hash, inline_contract_class). Phase 2 not needed.
    pub inline_already_built: Option<(String, ContractClass)>,
    /// Manifest parsed from Scarb.toml (needed for Phase 2 build)
    pub manifest: Manifest,
    /// Profile to build in Phase 2 (inline avoid strategy profile)
    pub inline_profile: String,
    /// Temporary directory containing the project files (kept alive until Phase 2 cleans up)
    pub tmp_dir: PathBuf,
    /// Source code cleaned of walnut-debug profile (for storing in cache)
    pub source_code: HashMap<String, String>,
    /// Contract name from Voyager (for multi-contract project selection)
    pub verified_name: String,
}

/// Phase 1: prepare source files and build non-inline profiles (release, dev) to find
/// the class matching the original on-chain hash.
///
/// Returns `Phase1Result` which always succeeds even if no profile matched the on-chain hash
/// (in that case `original_contract_class` is None). Errors only on preparation failure.
pub async fn compile_voyager_phase1(
    source_response: VoyagerSourceResponse,
    build_timeout: Option<Duration>,
) -> Result<Phase1Result> {
    let original_class_hash = source_response.class_hash.clone();
    let verified_name = source_response.verified_name.clone();

    let compiler_version = CompilerVersion::parse(&source_response.compiler_version)?;
    let version_tuple = compiler_version.as_tuple();

    if !is_new_cairo_version_supported(version_tuple) {
        return Err(anyhow::anyhow!(
            "Unsupported Cairo version {} from Voyager. Minimum supported is 2.8.2",
            source_response.compiler_version
        ));
    }

    let verification_id = format!("voyager-{}", Uuid::new_v4());
    let tmp_dir = PathBuf::from("tmp/verification").join(&verification_id);
    let tmp_dir_clone = tmp_dir.clone();
    let compiler_version_str = source_response.compiler_version.clone();

    info!(
        "Phase 1: Proceeding with Voyager source compilation for class {} in {}",
        original_class_hash,
        tmp_dir.display()
    );

    // Prepare: pin versions, parse manifest, write files to disk
    let prepare_result = tokio::task::spawn_blocking(move || {
        fs::create_dir_all(&tmp_dir_clone).map_err(|e| {
            anyhow::anyhow!(
                "Failed to create temp directory {}: {}",
                tmp_dir_clone.display(),
                e
            )
        })?;

        let mut source_code = source_response.source_code;
        pin_dependency_versions(&mut source_code, &compiler_version_str);

        let manifest = match Manifest::new_with_verified_name(
            &mut source_code,
            Some(version_tuple),
            Some(&source_response.verified_name),
        ) {
            Ok(m) => m,
            Err(e) => {
                let _ = fs::remove_dir_all(&tmp_dir_clone);
                return Err(anyhow::anyhow!("Failed to parse manifest: {}", e));
            }
        };

        if let Err(e) = create_files_from_map(&source_code, &tmp_dir_clone) {
            let _ = fs::remove_dir_all(&tmp_dir_clone);
            return Err(anyhow::anyhow!("Failed to write source files: {}", e));
        }

        let inline_profile = manifest
            .profile_with_inline_strategy
            .keys()
            .next()
            .cloned()
            .unwrap_or_else(|| "walnut-debug".to_string());

        // Clean source: remove walnut-debug profile added by manifest parsing
        let mut source_code_clean = source_code;
        remove_walnut_debug_from_scarb(&mut source_code_clean);

        Ok::<_, anyhow::Error>((manifest, inline_profile, source_code_clean))
    })
    .await?;

    let (manifest, inline_profile, source_code) = match prepare_result {
        Ok(r) => r,
        Err(e) => {
            // Clean up temp dir on preparation failure
            let tmp_dir_clone = tmp_dir.clone();
            let _ = tokio::task::spawn_blocking(move || cleanup_tmp_dir(&tmp_dir_clone)).await;
            return Err(anyhow::anyhow!("Phase 1 preparation failed: {}", e));
        }
    };

    // Proactively resolve transitive registry deps before the first build attempt.
    // This avoids a wasted first build that fails with a dep resolution error and
    // triggers the same fallback reactively (saving ~1s + the resolver round-trip).
    // If the resolver returns nothing or fails we proceed anyway; the reactive
    // fallback in the build loop will still trigger if needed.
    let mut dep_resolution_fallback_applied =
        match apply_dep_resolution_fallback(&manifest, &tmp_dir).await {
            Ok(()) => {
                warn!(
                    "Phase 1: proactive dep resolution applied for starknet {}.{}.{}",
                    manifest.cairo_version.0, manifest.cairo_version.1, manifest.cairo_version.2
                );
                true
            }
            Err(_) => false,
        };

    // Build release profile first, then dev if release doesn't match.
    // For each profile: if it matches AND has inline avoid strategy → both phases done at once.
    //
    // Reactive fallback: on the first dep-resolution error (only if proactive pass was not
    // applied) we query the scarbs.xyz registry API again and restart the profile loop once.
    let mut original_contract_class: Option<ContractClass> = None;
    let mut original_profile: Option<String> = None;
    let mut inline_already_built: Option<(String, ContractClass)> = None;

    'retry: loop {
        'outer: for non_inline_profile in &["release", "dev"] {
            // Skip if this profile is the designated inline profile
            // (we'd be building it again unnecessarily in phase 2 anyway)
            if *non_inline_profile == inline_profile.as_str() {
                continue;
            }

            debug!(
                "Phase 1: building '{}' profile for class {}",
                non_inline_profile, original_class_hash
            );

            match build_with_scarb_for_profile(
                &manifest,
                &tmp_dir,
                non_inline_profile,
                build_timeout,
            )
            .await
            {
                Ok(classes) if !classes.is_empty() => {
                    for (hash, contract_class) in classes {
                        if hash == original_class_hash {
                            info!(
                                "Phase 1: '{}' profile matched on-chain hash {}",
                                non_inline_profile, original_class_hash
                            );
                            original_contract_class = Some(contract_class.clone());
                            original_profile = Some((*non_inline_profile).to_string());

                            // Check if this profile already has inline avoid strategy
                            if manifest
                                .profile_with_inline_strategy
                                .contains_key(*non_inline_profile)
                            {
                                info!(
                                    "Phase 1: '{}' profile has inline strategy AND matches on-chain hash — no Phase 2 needed",
                                    non_inline_profile
                                );
                                inline_already_built = Some((hash, contract_class));
                            }
                            break 'outer;
                        }
                    }
                    warn!(
                        "Phase 1: '{}' profile produced classes but none matched on-chain hash {}",
                        non_inline_profile, original_class_hash
                    );
                }
                Ok(_) => {
                    warn!(
                        "Phase 1: Class {} with '{}' profile produced no classes",
                        original_class_hash, non_inline_profile
                    );
                }
                Err(e)
                    if !dep_resolution_fallback_applied
                        && is_dep_resolution_error(&e.to_string()) =>
                {
                    warn!(
                        "Phase 1: dep resolution error detected for class {} '{}' profile, triggering scarbs.xyz fallback. Error: {}",
                        original_class_hash, non_inline_profile, e
                    );
                    dep_resolution_fallback_applied = true;
                    match apply_dep_resolution_fallback(&manifest, &tmp_dir).await {
                        Ok(()) => {
                            debug!(
                                "Phase 1: scarbs.xyz dep resolution fallback applied, retrying build"
                            );
                            continue 'retry;
                        }
                        Err(fallback_err) => {
                            warn!(
                                "BUILD FAILED for class {} with '{}' profile: {}. Dep resolution fallback also failed: {}",
                                original_class_hash, non_inline_profile, e, fallback_err
                            );
                        }
                    }
                }
                Err(e) => {
                    warn!(
                        "BUILD FAILED for class {} with '{}' profile: {}",
                        original_class_hash, non_inline_profile, e
                    );
                }
            }
        }
        break 'retry;
    }

    if original_contract_class.is_none() {
        warn!(
            "Phase 1: no non-inline profile matched on-chain hash {} — function calls unavailable for simple trace",
            original_class_hash
        );
    }

    Ok(Phase1Result {
        original_class_hash,
        original_contract_class,
        original_profile,
        inline_already_built,
        manifest,
        inline_profile,
        tmp_dir,
        source_code,
        verified_name,
    })
}

/// Phase 2: build the inline (walnut-debug or equivalent) profile to get coverage annotations.
/// Consumes `Phase1Result` and cleans up the temp directory when done.
///
/// Only call this when `phase1.inline_already_built` is None.
pub async fn compile_voyager_phase2(
    phase1: Phase1Result,
    build_timeout: Option<Duration>,
) -> Result<CompiledExternalClass> {
    let tmp_dir = phase1.tmp_dir.clone();
    let original_class_hash = phase1.original_class_hash.clone();
    let verified_name = phase1.verified_name.clone();

    debug!(
        "Phase 2: building '{}' profile for class {}",
        phase1.inline_profile, original_class_hash
    );

    let compiled_classes = match build_with_scarb_for_profile(
        &phase1.manifest,
        &tmp_dir,
        &phase1.inline_profile,
        build_timeout,
    )
    .await
    {
        Ok(classes) => classes,
        Err(e) => {
            let tmp_dir_clone = tmp_dir.clone();
            let _ = tokio::task::spawn_blocking(move || {
                if let Err(move_err) = move_failed_verification_to_failed_tmp(&tmp_dir_clone) {
                    error!("Failed to move verification to failed tmp: {:?}", move_err);
                }
            })
            .await;
            return Err(anyhow::anyhow!("Phase 2 inline compilation failed: {}", e));
        }
    };

    if compiled_classes.is_empty() {
        cleanup_tmp_dir(&tmp_dir);
        return Err(anyhow::anyhow!(
            "No contracts found in Phase 2 inline compiled output"
        ));
    }

    // Find the matching inline class (by verified_name for multi-contract, or use the only one)
    let (inline_class_hash, contract_class) = if compiled_classes.len() == 1 {
        compiled_classes.into_iter().next().unwrap()
    } else {
        let verified_name_clone = verified_name.clone();
        let tmp_dir_clone = tmp_dir.clone();
        let package_name = phase1.manifest.package_name.clone();
        let inline_profile_clone = phase1.inline_profile.clone();
        let target_class_hash = tokio::task::spawn_blocking(move || {
            find_class_hash_by_contract_name(
                &tmp_dir_clone,
                &package_name,
                &inline_profile_clone,
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
                warn!(
                    "Phase 2: could not find contract '{}' in artifacts, using first class",
                    verified_name
                );
                compiled_classes.into_iter().next().unwrap()
            }
        }
    };

    // Cleanup temp directory
    if let Err(e) = tokio::task::spawn_blocking(move || {
        cleanup_tmp_dir(&tmp_dir);
    })
    .await
    {
        error!("Failed to cleanup temp dir task: {:?}", e);
    }

    info!(
        "Completed inline build for {} → inline hash {}",
        original_class_hash, inline_class_hash
    );

    Ok(CompiledExternalClass {
        original_class_hash,
        inline_class_hash,
        contract_class,
        original_contract_class: phase1.original_contract_class,
        source_code: phase1.source_code,
        cairo_version: phase1.manifest.cairo_version,
        package_name: phase1.manifest.package_name,
        inline_profile: phase1.inline_profile,
        original_profile: phase1.original_profile,
    })
}

/// Compile source code fetched from Voyager API (Phase 1 + Phase 2).
///
/// Used by the debug trace fallback path when no pre-compilation has happened.
pub async fn compile_voyager_source(
    source_response: VoyagerSourceResponse,
    build_timeout: Option<Duration>,
) -> Result<CompiledExternalClass> {
    let phase1 = compile_voyager_phase1(source_response, build_timeout).await?;

    if let Some((inline_class_hash, contract_class)) = phase1.inline_already_built.clone() {
        // Matching profile already had inline strategy — no Phase 2 needed
        let tmp_dir = phase1.tmp_dir.clone();
        if let Err(e) = tokio::task::spawn_blocking(move || {
            cleanup_tmp_dir(&tmp_dir);
        })
        .await
        {
            error!("Failed to cleanup temp dir task: {:?}", e);
        }

        info!(
            "Compile_voyager_source: both phases done from single build for {}",
            phase1.original_class_hash
        );

        Ok(CompiledExternalClass {
            original_class_hash: phase1.original_class_hash,
            inline_class_hash,
            contract_class,
            original_contract_class: phase1.original_contract_class,
            source_code: phase1.source_code,
            cairo_version: phase1.manifest.cairo_version,
            package_name: phase1.manifest.package_name,
            inline_profile: phase1.inline_profile,
            original_profile: phase1.original_profile,
        })
    } else {
        compile_voyager_phase2(phase1, build_timeout).await
    }
}

/// Clean up temporary directory.
/// Tries `fs::remove_dir_all` first; on failure, moves the directory to
/// `tmp/failed-verification` for post-mortem investigation.
pub fn cleanup_tmp_dir(tmp_dir: &PathBuf) {
    if let Err(e) = fs::remove_dir_all(tmp_dir) {
        warn!(
            "Failed to clean up temp directory {}: {:?}, moving to failed-verification",
            tmp_dir.display(),
            e
        );
        if let Err(move_err) = move_failed_verification_to_failed_tmp(tmp_dir) {
            error!(
                "Failed to move {} to failed-verification: {:?}",
                tmp_dir.display(),
                move_err
            );
        }
    }
}

/// Calls the scarbs.xyz registry API to resolve all transitive registry dependencies
/// to exact versions compatible with the project's starknet/Cairo version, then injects
/// the resolved `name = "=X.Y.Z"` entries into the Scarb.toml on disk.
///
/// Only adds packages that are not already declared — existing entries are not overwritten
/// so that pinned deps (e.g. starknet = "=2.11.4") set by earlier passes are preserved.
async fn apply_dep_resolution_fallback(manifest: &Manifest, tmp_dir: &Path) -> Result<()> {
    let toml_path = tmp_dir.join("Scarb.toml");
    let content = fs::read_to_string(&toml_path)?;
    let starknet_version = format!(
        "{}.{}.{}",
        manifest.cairo_version.0, manifest.cairo_version.1, manifest.cairo_version.2
    );

    debug!(
        "Querying scarbs.xyz registry to resolve deps for starknet {}",
        starknet_version
    );

    let resolved = resolve_registry_deps(&content, &starknet_version)
        .await
        .map_err(|e| anyhow::anyhow!("scarbs.xyz dep resolution failed: {}", e))?;

    if resolved.is_empty() {
        return Err(anyhow::anyhow!(
            "scarbs.xyz dep resolution returned no packages"
        ));
    }

    let mut toml: toml::Value = content
        .parse()
        .map_err(|e| anyhow::anyhow!("Failed to parse Scarb.toml: {}", e))?;

    if let Some(toml::Value::Table(deps)) =
        toml.as_table_mut().and_then(|t| t.get_mut("dependencies"))
    {
        for (name, version) in &resolved {
            // Do not override an existing explicit declaration
            if !deps.contains_key(name.as_str()) {
                deps.insert(name.clone(), toml::Value::String(version.clone()));
            }
        }
    }

    fs::write(&toml_path, toml::to_string_pretty(&toml)?)?;
    warn!(
        "scarbs.xyz fallback: injected {} pinned packages into Scarb.toml",
        resolved.len()
    );
    Ok(())
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

    // Pin [dependencies].starknet to exact compiler version.
    // Handles both explicit ranges (>=, ^, etc.) and bare versions ("2.11.4" == ^2.11.4 in Scarb).
    if let Some(starknet) = toml_value
        .get_mut("dependencies")
        .and_then(|d| d.as_table_mut())
        .and_then(|deps| deps.get_mut("starknet"))
    {
        if let Some(version_str) = starknet.as_str() {
            if is_version_range(version_str) || is_plain_version(version_str) {
                *starknet = toml::Value::String(format!("={}", compiler_version));
                modified = true;
            }
        }
    }

    // Pin all other plain version string dependencies to exact.
    // In Scarb, "2.0.0" is treated as ^2.0.0 and resolves to the latest compatible version
    // (e.g. openzeppelin "2.0.0" may resolve to 2.1.0 which requires a newer starknet).
    if let Some(deps) = toml_value
        .get_mut("dependencies")
        .and_then(|d| d.as_table_mut())
    {
        for (name, value) in deps.iter_mut() {
            if name == "starknet" {
                continue; // already handled above
            }
            if let Some(version_str) = value.as_str() {
                if is_plain_version(version_str) {
                    *value = toml::Value::String(format!("={}", version_str));
                    modified = true;
                }
            }
        }
    }

    // Pin [package].cairo-version to exact compiler version.
    // Same logic: bare "2.11.4" is treated as ^2.11.4 by Scarb.
    if let Some(cairo_ver) = toml_value
        .get_mut("package")
        .and_then(|p| p.as_table_mut())
        .and_then(|pkg| pkg.get_mut("cairo-version"))
    {
        if let Some(ver_str) = cairo_ver.as_str() {
            if is_version_range(ver_str) || is_plain_version(ver_str) {
                *cairo_ver = toml::Value::String(format!("={}", compiler_version));
                modified = true;
            }
        }
    }

    // Remove [dev-dependencies] — can't be resolved without Scarb.lock
    // (e.g. snforge_std = ">=0.55.0")
    if let Some(table) = toml_value.as_table_mut() {
        if table.remove("dev-dependencies").is_some() {
            modified = true;
        }
        if table.remove("scripts").is_some() {
            modified = true;
        }
    }

    if modified {
        if let Ok(updated) = toml::to_string(&toml_value) {
            source_code.insert("Scarb.toml".to_string(), updated);
            debug!(
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

/// Returns true for bare version strings like "2.0.0" (no operator prefix).
/// Scarb treats these as ^X.Y.Z (compatible with major), so without a Scarb.lock
/// the resolver can pick a newer minor version that may require a newer starknet.
fn is_plain_version(version_str: &str) -> bool {
    version_str
        .chars()
        .next()
        .map(|c| c.is_ascii_digit())
        .unwrap_or(false)
}
