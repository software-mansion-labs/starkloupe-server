use crate::external_class_cache::ExternalClassCache;
use crate::{ClassDebuggerData, ClassDebuggerDataWithContractClass, DataWithContractClass};
use anyhow::Result;
use cairo_annotations::annotations::coverage::VersionedCoverageAnnotations;
use cairo_annotations::annotations::TryFromDebugInfo;
use cairo_lang_starknet_classes::contract_class::ContractClass;
use futures::future;
use futures::stream::{self, StreamExt};
use futures::FutureExt;
use itertools::Itertools;
use sqlx::{Pool, Postgres};
use std::collections::{HashMap, HashSet};
use tracing::{debug, error, info, warn};
use verification::{
    db::fetch_verified_classes_with_inlining_classes,
    s3::key_for_class_hash,
    voyager::{
        cleanup_tmp_dir, compile_voyager_phase1, compile_voyager_phase2, compile_voyager_source,
        VoyagerClient,
    },
    CodeLocation, SierraStatementToCairoDebugInfo, VerifiedClassData,
};

async fn fetch_and_parse_file(
    client: &aws_sdk_s3::Client,
    bucket_name: &str,
    key: String,
) -> Result<VerifiedClassData> {
    let resp = client
        .get_object()
        .bucket(bucket_name)
        .key(key)
        .send()
        .await?;

    let body = resp.body.collect().await?;
    let parsed: VerifiedClassData = serde_json::from_slice(&body.into_bytes())?;
    Ok(parsed)
}

pub async fn fetch_classes_data(
    db_pool: &Pool<Postgres>,
    s3_client: &aws_sdk_s3::Client,
    classes: &[String],
) -> HashMap<String, DataWithContractClass> {
    let mut classes_debugger_data: HashMap<String, DataWithContractClass> = HashMap::new();

    let verified_classes =
        match fetch_verified_classes_with_inlining_classes(db_pool, classes).await {
            Ok(vc) => vc,
            Err(e) => {
                warn!("Failed to fetch verified classes: {:?}", e);
                HashMap::new()
            }
        };

    let fetches = verified_classes.keys().map(|key| {
        let key = key.clone();
        async move {
            match fetch_and_parse_file(
                s3_client,
                "walnutserver-east-1-classes-verification",
                key_for_class_hash(&key),
            )
            .await
            {
                Ok(parsed) => (key, Some(parsed)),
                Err(err) => {
                    warn!("Failed to fetch or parse for key {}: {:?}", key, err);
                    (key, None)
                }
            }
        }
    });

    let results: HashMap<String, VerifiedClassData> = future::join_all(fetches)
        .await
        .into_iter()
        .filter_map(|(key, data)| data.map(|d| (key, d)))
        .collect();

    for (class_hash, inline_strategy_class_hash) in verified_classes.iter() {
        if let Some(verified_class_data) = results.get(class_hash) {
            classes_debugger_data.insert(
                class_hash.clone(),
                DataWithContractClass {
                    inline_strategy_class_hash: inline_strategy_class_hash.clone(),
                    contract_class: verified_class_data.contract_class.clone(),
                },
            );
        }
    }

    classes_debugger_data
}

/// Fetches the debugger data for the given classes with optional external verification fallback.
///
/// # Arguments
/// * `db_pool` - Database connection pool
/// * `s3_client` - S3 client for fetching verified class data
/// * `classes` - List of class hashes to fetch
/// * `external_cache` - Optional cache for externally compiled classes
/// * `voyager_client` - Optional Voyager API client for fetching unverified classes
pub async fn fetch_classes_debugger_data_with_external(
    db_pool: &Pool<Postgres>,
    s3_client: &aws_sdk_s3::Client,
    classes: &[String],
    external_cache: Option<&ExternalClassCache>,
    voyager_client: Option<&VoyagerClient>,
) -> HashMap<String, ClassDebuggerDataWithContractClass> {
    let mut classes_debugger_data: HashMap<String, ClassDebuggerDataWithContractClass> =
        HashMap::new();

    let verified_classes =
        match fetch_verified_classes_with_inlining_classes(db_pool, classes).await {
            Ok(vc) => vc,
            Err(e) => {
                warn!("Failed to fetch verified classes: {:?}", e);
                HashMap::new()
            }
        };

    let fetches = verified_classes
        .iter()
        .map(|(key, value)| {
            let fetch = match value {
                Some(value) => fetch_and_parse_file(
                    s3_client,
                    "walnutserver-east-1-classes-verification",
                    key_for_class_hash(value),
                ),
                None => fetch_and_parse_file(
                    s3_client,
                    "walnutserver-east-1-classes-verification",
                    key_for_class_hash(key),
                ),
            };
            let key = key.clone();
            fetch.map(|res| match res {
                Ok(data) => (key, Some(data)),
                Err(e) => {
                    warn!("Failed to fetch file: {:?}", e);
                    (key, None)
                }
            })
        })
        .collect::<Vec<_>>();

    let results: HashMap<String, VerifiedClassData> = future::join_all(fetches)
        .await
        .into_iter()
        .filter_map(|(key, data)| data.map(|d| (key, d)))
        .collect();

    for (class_hash, inline_class_hash) in verified_classes.iter() {
        if let Some(verified_class_data) = results.get(class_hash) {
            let class_debugger_data = if let Some(cairo_debug_info) =
                &verified_class_data.cairo_debug_info
            {
                Some(ClassDebuggerData {
                    sierra_statements_to_cairo_info: cairo_debug_info
                        .sierra_statements_to_cairo_info
                        .clone(),
                    source_code: verified_class_data.source_code.clone(),
                })
            } else {
                if let Some(debug_info) =
                    &verified_class_data.contract_class.sierra_program_debug_info
                {
                    match VersionedCoverageAnnotations::try_from_debug_info(debug_info) {
                        Ok(annotations) => match annotations {
                            VersionedCoverageAnnotations::V1(annotations) => {
                                let mut sierra_statements_to_cairo_info: HashMap<
                                    usize,
                                    SierraStatementToCairoDebugInfo,
                                > = HashMap::new();
                                for (id, code_locations) in annotations.statements_code_locations {
                                    sierra_statements_to_cairo_info.insert(
                                        id.0,
                                        SierraStatementToCairoDebugInfo {
                                            cairo_locations: code_locations
                                                .into_iter()
                                                .filter_map(|c| {
                                                    let mut code_location =
                                                        CodeLocation::from_coverage(c);
                                                    if code_location
                                                        .file_path
                                                        .contains("tmp/verification")
                                                    {
                                                        if code_location
                                                            .file_path
                                                            .ends_with("[contract]")
                                                        {
                                                            code_location.file_path = code_location
                                                                .file_path
                                                                .replace("[contract]", "");
                                                        }
                                                        if let Some(pos) = code_location
                                                            .file_path
                                                            .find("tmp/verification/")
                                                        {
                                                            let after_tmp = &code_location
                                                                .file_path
                                                                [pos + "tmp/verification/".len()..];
                                                            if let Some(slash_pos) =
                                                                after_tmp.find('/')
                                                            {
                                                                code_location.file_path = after_tmp
                                                                    [slash_pos + 1..]
                                                                    .to_string();
                                                            }
                                                        }
                                                        Some(code_location)
                                                    } else {
                                                        None
                                                    }
                                                })
                                                .collect_vec(),
                                        },
                                    );
                                }
                                Some(ClassDebuggerData {
                                    sierra_statements_to_cairo_info,
                                    source_code: verified_class_data.source_code.clone(),
                                })
                            }
                        },
                        Err(e) => {
                            error!("Failed to parse coverage info: {:?}", e);
                            None
                        }
                    }
                } else {
                    None
                }
            };

            classes_debugger_data.insert(
                class_hash.clone(),
                ClassDebuggerDataWithContractClass {
                    inline_strategy_class_hash: inline_class_hash.clone(),
                    class_debugger_data,
                    contract_class: verified_class_data.contract_class.clone(),
                },
            );
        }
    }

    // Voyager fallback for missing classes
    if external_cache.is_some() || voyager_client.is_some() {
        let missing_classes: Vec<String> = classes
            .iter()
            .filter(|c| !classes_debugger_data.contains_key(*c))
            .cloned()
            .collect();

        if !missing_classes.is_empty() {
            info!(
                "Classes not found in Walnut DB, checking Voyager: {:?}",
                missing_classes
            );
        }

        for class_hash in missing_classes {
            if class_hash
                == "0x0000000000000000000000000000000000000000000000000000000000000117"
            {
                continue;
            }

            // 1. Check external cache
            if let Some(cache) = external_cache {
                if let Some(cached) = cache.get(&class_hash).await {
                    match &cached.data {
                        Some(data) => {
                            // Phase 2 complete — inline data available
                            debug!(
                                "Found {} in external cache with inline data (source: {})",
                                class_hash, cached.source
                            );
                            classes_debugger_data.insert(class_hash.clone(), data.clone());
                            continue;
                        }
                        None => {
                            // Phase 1 done but Phase 2 still in progress
                            if cache.is_compiling(&class_hash).await {
                                info!(
                                    "Phase 2 in progress for {}, waiting for inline data",
                                    class_hash
                                );
                                cache.wait_for_pending(&class_hash).await;

                                if let Some(cached) = cache.get(&class_hash).await {
                                    if let Some(data) = &cached.data {
                                        classes_debugger_data
                                            .insert(class_hash.clone(), data.clone());
                                    }
                                }
                            }
                            // If not compiling but data=None → Phase 2 failed, fall through
                            // to try Voyager API for a fresh compile
                            if classes_debugger_data.contains_key(&class_hash) {
                                continue;
                            }
                        }
                    }
                } else {
                    // Cache miss — check if compilation previously failed
                    if cache.has_failed(&class_hash).await {
                        debug!("Skipping {} - compilation previously failed", class_hash);
                        continue;
                    }

                    // Wait if Phase 1 is currently in progress (compilation just started)
                    if cache.is_compiling(&class_hash).await {
                        info!(
                            "Compilation in progress for {}, waiting for full completion",
                            class_hash
                        );
                        cache.wait_for_pending(&class_hash).await;

                        if let Some(cached) = cache.get(&class_hash).await {
                            if let Some(data) = &cached.data {
                                classes_debugger_data.insert(class_hash.clone(), data.clone());
                            }
                        }
                        continue;
                    }
                }
            }

            // 2. Try Voyager API if available
            if let Some(client) = voyager_client {
                if !client.is_enabled() {
                    continue;
                }

                debug!("Fetching source from Voyager for class {}", class_hash);
                match client.fetch_source_code(&class_hash).await {
                    Ok(Some(source_response)) => {
                        info!(
                            "Fetched source from Voyager for {} ({})",
                            class_hash, source_response.verified_name
                        );

                        let notifier = if let Some(cache) = external_cache {
                            cache.start_compilation(&class_hash).await
                        } else {
                            None
                        };

                        match compile_voyager_source(source_response).await {
                            Ok(compiled) => {
                                let class_debugger_data = extract_debugger_data_from_contract_class(
                                    &compiled.contract_class,
                                    &compiled.source_code,
                                );

                                let data = ClassDebuggerDataWithContractClass {
                                    inline_strategy_class_hash: Some(
                                        compiled.inline_class_hash.clone(),
                                    ),
                                    class_debugger_data,
                                    contract_class: compiled.contract_class,
                                };

                                if let Some(cache) = external_cache {
                                    cache
                                        .set(
                                            &compiled.original_class_hash,
                                            Some(data.clone()),
                                            compiled.original_contract_class,
                                            Some(compiled.inline_class_hash),
                                            "voyager",
                                        )
                                        .await;
                                }

                                classes_debugger_data
                                    .insert(compiled.original_class_hash, data);

                                info!(
                                    "Successfully compiled Voyager source for {}",
                                    class_hash
                                );
                            }
                            Err(e) => {
                                warn!(
                                    "Failed to compile Voyager source for {}: {:?}",
                                    class_hash, e
                                );
                                if let Some(cache) = external_cache {
                                    cache.mark_failed(&class_hash).await;
                                }
                            }
                        }

                        if let Some(cache) = external_cache {
                            if let Some((phase1_notifier, phase2_notifier)) = notifier {
                                cache
                                    .signal_phase1_ready(&class_hash, phase1_notifier)
                                    .await;
                                cache
                                    .finish_compilation(&class_hash, phase2_notifier)
                                    .await;
                            }
                        }
                    }
                    Ok(None) | Err(_) => {}
                }
            }
        }
    }

    classes_debugger_data
}

/// Checks which class hashes are verified on Voyager and triggers two-phase background
/// pre-compilation.
///
/// Phase 1 (non-inline build): builds release/dev profiles to find `original_contract_class`.
/// Signals `phase1_ready` when done → simple trace can get function calls.
///
/// Phase 2 (inline build): builds walnut-debug (or equivalent) for coverage annotations.
/// Signals `pending_compilations` when done → debug trace can get source mapping.
///
/// If the matching profile already has inline avoid strategy, both phases complete at once.
///
/// Returns a set of class hashes that are verified on Voyager.
pub async fn check_voyager_verified_classes(
    voyager_client: Option<&VoyagerClient>,
    class_hashes: &[String],
    already_verified: &HashSet<String>,
    external_cache: Option<&ExternalClassCache>,
) -> HashSet<String> {
    let mut voyager_verified = HashSet::new();

    let client = match voyager_client {
        Some(c) if c.is_enabled() => c,
        _ => return voyager_verified,
    };

    let mut classes_to_check = Vec::new();
    for class_hash in class_hashes {
        if class_hash == "0x0000000000000000000000000000000000000000000000000000000000000117" {
            continue;
        }
        if already_verified.contains(class_hash) {
            continue;
        }
        if let Some(cache) = external_cache {
            if cache.contains(class_hash).await {
                voyager_verified.insert(class_hash.clone());
                continue;
            }
            if cache.has_failed(class_hash).await {
                continue;
            }
        }
        classes_to_check.push(class_hash.clone());
    }

    if classes_to_check.is_empty() {
        return voyager_verified;
    }

    debug!(
        "Checking Voyager verification for {} classes in parallel",
        classes_to_check.len()
    );

    let concurrency_limit = std::env::var("VOYAGER_CONCURRENCY_LIMIT")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(5);

    let results: Vec<_> = stream::iter(classes_to_check)
        .map(|class_hash| async move {
            let result = client.fetch_source_code(&class_hash).await;
            (class_hash, result)
        })
        .buffer_unordered(concurrency_limit)
        .collect()
        .await;

    for (class_hash, result) in results {
        if let Ok(Some(source_response)) = result {
            voyager_verified.insert(class_hash.clone());

            if let Some(cache) = external_cache {
                let cache = cache.clone();
                let class_hash_clone = class_hash.clone();

                if let Some((phase1_notifier, phase2_notifier)) =
                    cache.start_compilation(&class_hash).await
                {
                    tokio::spawn(async move {
                        // Phase 1: non-inline build (release / dev)
                        let phase1 =
                            match compile_voyager_phase1(source_response).await {
                                Ok(p) => p,
                                Err(e) => {
                                    warn!(
                                        "Phase 1 failed for {}: {:?}",
                                        class_hash_clone, e
                                    );
                                    cache.mark_failed(&class_hash_clone).await;
                                    // Signal both so waiters don't hang
                                    cache
                                        .signal_phase1_ready(&class_hash_clone, phase1_notifier)
                                        .await;
                                    cache
                                        .finish_compilation(&class_hash_clone, phase2_notifier)
                                        .await;
                                    return;
                                }
                            };

                        let original_class_hash = phase1.original_class_hash.clone();

                        // Check if inline is already available from Phase 1
                        if let Some((ref inline_hash, ref inline_class)) =
                            phase1.inline_already_built
                        {
                            // Matching profile had inline strategy — both phases done at once
                            let class_debugger_data = extract_debugger_data_from_contract_class(
                                inline_class,
                                &phase1.source_code,
                            );
                            let data = ClassDebuggerDataWithContractClass {
                                inline_strategy_class_hash: Some(inline_hash.clone()),
                                class_debugger_data,
                                contract_class: inline_class.clone(),
                            };

                            cache
                                .set(
                                    &original_class_hash,
                                    Some(data),
                                    phase1.original_contract_class.clone(),
                                    Some(inline_hash.clone()),
                                    "voyager-phase1+inline",
                                )
                                .await;

                            // Cleanup temp dir (phase2 won't do it)
                            let tmp_dir = phase1.tmp_dir.clone();
                            let _ = tokio::task::spawn_blocking(move || {
                                cleanup_tmp_dir(&tmp_dir);
                            })
                            .await;

                            info!(
                                "Both phases complete (single build) for {}",
                                class_hash_clone
                            );

                            cache
                                .signal_phase1_ready(&class_hash_clone, phase1_notifier)
                                .await;
                            cache
                                .finish_compilation(&class_hash_clone, phase2_notifier)
                                .await;
                        } else {
                            // Set Phase 1 result in cache (no inline data yet)
                            cache
                                .set(
                                    &original_class_hash,
                                    None,
                                    phase1.original_contract_class.clone(),
                                    None,
                                    "voyager-phase1",
                                )
                                .await;

                            // Signal Phase 1 complete → unblocks simple trace
                            cache
                                .signal_phase1_ready(&class_hash_clone, phase1_notifier)
                                .await;

                            info!(
                                "Phase 1 complete for {}, starting Phase 2 in background",
                                class_hash_clone
                            );

                            // Phase 2: inline build (walnut-debug or equivalent profile)
                            match compile_voyager_phase2(phase1).await {
                                Ok(compiled) => {
                                    let class_debugger_data =
                                        extract_debugger_data_from_contract_class(
                                            &compiled.contract_class,
                                            &compiled.source_code,
                                        );
                                    let data = ClassDebuggerDataWithContractClass {
                                        inline_strategy_class_hash: Some(
                                            compiled.inline_class_hash.clone(),
                                        ),
                                        class_debugger_data,
                                        contract_class: compiled.contract_class,
                                    };
                                    cache
                                        .update_inline_data(
                                            &compiled.original_class_hash,
                                            data,
                                            Some(compiled.inline_class_hash),
                                        )
                                        .await;
                                    info!(
                                        "Phase 2 complete for {}",
                                        class_hash_clone
                                    );
                                }
                                Err(e) => {
                                    warn!(
                                        "Phase 2 failed for {}: {:?}",
                                        class_hash_clone, e
                                    );
                                    // Don't mark_failed — Phase 1 data (original_contract_class)
                                    // is still valid for simple trace function calls
                                }
                            }

                            // Signal Phase 2 complete → unblocks debug trace
                            cache
                                .finish_compilation(&class_hash_clone, phase2_notifier)
                                .await;
                        }
                    });
                }
            }
        }
    }

    voyager_verified
}

/// Extract debugger data (source mapping) from a compiled contract class
fn extract_debugger_data_from_contract_class(
    contract_class: &ContractClass,
    source_code: &HashMap<String, String>,
) -> Option<ClassDebuggerData> {
    let debug_info = contract_class.sierra_program_debug_info.as_ref()?;

    match VersionedCoverageAnnotations::try_from_debug_info(debug_info) {
        Ok(annotations) => match annotations {
            VersionedCoverageAnnotations::V1(annotations) => {
                let mut sierra_statements_to_cairo_info: HashMap<
                    usize,
                    SierraStatementToCairoDebugInfo,
                > = HashMap::new();

                for (id, code_locations) in annotations.statements_code_locations {
                    sierra_statements_to_cairo_info.insert(
                        id.0,
                        SierraStatementToCairoDebugInfo {
                            cairo_locations: code_locations
                                .into_iter()
                                .filter_map(|c| {
                                    let mut code_location = CodeLocation::from_coverage(c);
                                    if code_location.file_path.contains("tmp/verification") {
                                        if code_location.file_path.ends_with("[contract]") {
                                            code_location.file_path =
                                                code_location.file_path.replace("[contract]", "");
                                        }
                                        if let Some(pos) =
                                            code_location.file_path.find("tmp/verification/")
                                        {
                                            let after_tmp = &code_location.file_path
                                                [pos + "tmp/verification/".len()..];
                                            if let Some(slash_pos) = after_tmp.find('/') {
                                                code_location.file_path =
                                                    after_tmp[slash_pos + 1..].to_string();
                                            }
                                        }
                                        Some(code_location)
                                    } else {
                                        None
                                    }
                                })
                                .collect_vec(),
                        },
                    );
                }

                Some(ClassDebuggerData {
                    sierra_statements_to_cairo_info,
                    source_code: source_code.clone(),
                })
            }
        },
        Err(e) => {
            warn!("Failed to extract coverage annotations: {:?}", e);
            None
        }
    }
}
