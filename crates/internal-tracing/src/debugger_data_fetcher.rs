use crate::external_class_cache::ExternalClassCache;
use crate::{ClassDebuggerData, ClassDebuggerDataWithContractClass, DataWithContractClass};
use anyhow::Result;
use cairo_annotations::annotations::coverage::VersionedCoverageAnnotations;
use cairo_annotations::annotations::TryFromDebugInfo;
use cairo_lang_starknet_classes::contract_class::ContractClass;
use futures::future;
use futures::FutureExt;
use itertools::Itertools;
use sqlx::{Pool, Postgres};
use std::collections::{HashMap, HashSet};
use tracing::{debug, error, info, warn};
use verification::{
    db::fetch_verified_classes_with_inlining_classes,
    s3::key_for_class_hash,
    voyager::{compile_voyager_source, VoyagerClient},
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

/// Fetches the debugger data for the given classes.
///
/// This function first checks the Walnut DB for verified classes,
/// then optionally falls back to Voyager API for missing classes.
//pub async fn fetch_classes_debugger_data(
//    db_pool: &Pool<Postgres>,
//    s3_client: &aws_sdk_s3::Client,
//    classes: &[String],
//) -> HashMap<String, ClassDebuggerDataWithContractClass> {
//    info!("Fetching debugger data for the classes {:?}", classes);
//    fetch_classes_debugger_data_with_external(db_pool, s3_client, classes, None, None).await
//}

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
    info!("Fetching debugger data for classes {:?}", classes);
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
                                                        // Extract path after tmp/verification/<id>/
                                                        // e.g., /tmp/verification/abc123/layerzero/src/file.cairo
                                                        // becomes layerzero/src/file.cairo
                                                        if let Some(pos) =
                                                            code_location.file_path.find("tmp/verification/")
                                                        {
                                                            let after_tmp = &code_location.file_path[pos + "tmp/verification/".len()..];
                                                            // Skip the verification ID (first path segment)
                                                            if let Some(slash_pos) = after_tmp.find('/') {
                                                                code_location.file_path = after_tmp[slash_pos + 1..].to_string();
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

    info!("Some classes are not verified on Walnut, let check voyager api");
    // Voyager fallback for missing classes
    if external_cache.is_some() || voyager_client.is_some() {
        let missing_classes: Vec<String> = classes
            .iter()
            .filter(|c| !classes_debugger_data.contains_key(*c))
            .cloned()
            .collect();

        info!("missing_classes {:?}", missing_classes);
        if !missing_classes.is_empty() {
            debug!(
                "Attempting Voyager fallback for {} missing classes",
                missing_classes.len()
            );

            for class_hash in missing_classes {
                // 1. Check external cache first
                if let Some(cache) = external_cache {
                    if let Some(cached) = cache.get(&class_hash).await {
                        debug!(
                            "Found {} in external cache (source: {})",
                            class_hash, cached.source
                        );
                        classes_debugger_data.insert(class_hash.clone(), cached.data.clone());
                        continue;
                    }
                }

                // 2. Try Voyager API if available
                if let Some(client) = voyager_client {
                    if !client.is_enabled() {
                        continue;
                    }

                    match client.fetch_source_code(&class_hash).await {
                        Ok(Some(source_response)) => {
                            info!(
                                "Fetched source from Voyager for {} ({})",
                                class_hash, source_response.verified_name
                            );

                            // 3. Compile the source code
                            match compile_voyager_source(source_response).await {
                                Ok(compiled) => {
                                    // 4. Extract debugger data from compiled class
                                    let class_debugger_data =
                                        extract_debugger_data_from_contract_class(
                                            &compiled.contract_class,
                                            &compiled.source_code,
                                        );

                                    let data = ClassDebuggerDataWithContractClass {
                                        inline_strategy_class_hash: Some(
                                            compiled.inline_class_hash,
                                        ),
                                        class_debugger_data,
                                        contract_class: compiled.contract_class,
                                    };

                                    // 5. Cache the result
                                    if let Some(cache) = external_cache {
                                        cache
                                            .set(
                                                &compiled.original_class_hash,
                                                data.clone(),
                                                "voyager",
                                            )
                                            .await;
                                    }

                                    // 6. Add to result map with ORIGINAL class hash
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
                                }
                            }
                        }
                        Ok(None) => {
                            debug!("Class {} not found on Voyager", class_hash);
                        }
                        Err(e) => {
                            warn!("Failed to fetch from Voyager for {}: {:?}", class_hash, e);
                        }
                    }
                }
            }
        }
    }

    classes_debugger_data
}

/// Checks which class hashes are verified on Voyager and triggers background
/// pre-compilation for the debug path.
///
/// Returns a set of class hashes that are verified on Voyager.
/// This is used in the simulate (non-debug) path to:
/// 1. Determine if the green debug button should be shown in the UI
/// 2. Pre-compile Voyager sources in the background so that when the user
///    clicks "debug", the compiled class is already in the cache
///
/// This way the simulate request stays fast (just pings Voyager), while the
/// debug request benefits from the pre-compiled cache hit.
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

    let missing_classes: Vec<&String> = class_hashes
        .iter()
        .filter(|h| !already_verified.contains(*h))
        .collect();

    if missing_classes.is_empty() {
        return voyager_verified;
    }

    debug!(
        "Checking Voyager verification for {} missing classes",
        missing_classes.len()
    );

    for class_hash in missing_classes {
        // Skip if already in external cache (already compiled)
        if let Some(cache) = external_cache {
            if cache.contains(class_hash).await {
                debug!("Class {} already in external cache, skipping Voyager check", class_hash);
                voyager_verified.insert(class_hash.clone());
                continue;
            }
        }

        match client.fetch_source_code(class_hash).await {
            Ok(Some(source_response)) => {
                info!(
                    "Class {} is verified on Voyager (simulate path, name: {})",
                    class_hash, source_response.verified_name
                );
                voyager_verified.insert(class_hash.clone());

                // Spawn background compilation so debug request gets a cache hit
                if let Some(cache) = external_cache {
                    let cache = cache.clone();
                    let class_hash = class_hash.clone();
                    tokio::spawn(async move {
                        info!(
                            "Background pre-compilation started for Voyager class {}",
                            class_hash
                        );
                        match compile_voyager_source(source_response).await {
                            Ok(compiled) => {
                                let class_debugger_data =
                                    extract_debugger_data_from_contract_class(
                                        &compiled.contract_class,
                                        &compiled.source_code,
                                    );

                                let data = ClassDebuggerDataWithContractClass {
                                    inline_strategy_class_hash: Some(compiled.inline_class_hash),
                                    class_debugger_data,
                                    contract_class: compiled.contract_class,
                                };

                                cache
                                    .set(&compiled.original_class_hash, data, "voyager-precompile")
                                    .await;

                                info!(
                                    "Background pre-compilation completed for Voyager class {}",
                                    class_hash
                                );
                            }
                            Err(e) => {
                                warn!(
                                    "Background pre-compilation failed for {}: {:?}",
                                    class_hash, e
                                );
                            }
                        }
                    });
                }
            }
            Ok(None) => {
                debug!("Class {} not found on Voyager", class_hash);
            }
            Err(e) => {
                warn!("Failed to check Voyager for class {}: {:?}", class_hash, e);
            }
        }
    }

    voyager_verified
}

/// Extract debugger data from a compiled contract class
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
                                    // Normalize file paths from tmp/verification
                                    if code_location.file_path.contains("tmp/verification") {
                                        if code_location.file_path.ends_with("[contract]") {
                                            code_location.file_path =
                                                code_location.file_path.replace("[contract]", "");
                                        }
                                        // Extract path after tmp/verification/<id>/
                                        // e.g., /tmp/verification/voyager-abc123/layerzero/src/file.cairo
                                        // becomes layerzero/src/file.cairo
                                        if let Some(pos) = code_location.file_path.find("tmp/verification/") {
                                            let after_tmp = &code_location.file_path[pos + "tmp/verification/".len()..];
                                            // Skip the verification ID (first path segment)
                                            if let Some(slash_pos) = after_tmp.find('/') {
                                                code_location.file_path = after_tmp[slash_pos + 1..].to_string();
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
