use crate::db::{
    fetch_inline_class_hash_profiles_by_class_hash, fetch_verification_statuses_pending_or_success,
    fetch_verified_class, fetch_verified_classes, insert_class_hash_profiles,
};
use crate::manifest::Manifest;
use crate::scarb::{
    build_with_scarb_for_profile, compile_with_scarb_for_profile, is_new_cairo_version_supported,
};
use crate::utils::move_failed_verification_to_failed_tmp;
use crate::SierraToCairoDebugInfo;
use crate::{ClassVerificationData, EVerificationStatus};
use anyhow::{Context, Result};
use cairo_lang_starknet_classes::contract_class::ContractClass;
use sqlx::{Pool, Postgres};
use starknet::core::types::{BlockId, BlockTag, ContractClass as CoreContractClass, Felt};
use starknet::providers::{
    jsonrpc::{HttpTransport, JsonRpcClient},
    Provider,
};
use std::collections::HashSet;
use std::io::BufReader;
use std::path::PathBuf;
use std::str::FromStr;
use std::{collections::HashMap, fs::File};
use tracing::error;
use tracing::info;
use uuid::Uuid;
use walnut_shared::extract_cairo_version_from_program;

pub fn initialize_status_map(
    class_hashes: &[String],
    class_names: &[String],
) -> HashMap<String, (String, EVerificationStatus, Option<String>)> {
    class_hashes
        .iter()
        .zip(class_names.iter().cloned())
        .map(|(hash, name)| (hash.clone(), (name, EVerificationStatus::Pending, None)))
        .collect()
}

pub async fn update_status_from_verification_table_with_inline_class_hash(
    db_pool: &Pool<Postgres>,
    class_hashes: &[String],
    class_hash_to_version: &HashMap<String, (u32, u32, u32)>,
    inline_map: &HashMap<String, Option<String>>,
    class_status_map: &mut HashMap<String, (String, EVerificationStatus, Option<String>)>,
) -> Result<()> {
    // Fetch statuses za *samo* korisničke class_hashes
    let statuses = fetch_verification_statuses_pending_or_success(db_pool, class_hashes).await?;
    let status_map: HashMap<_, _> = statuses.iter().cloned().collect();

    // Za inline class hashes treba da fetchujemo statuse posebno - sve inline hash-eve koje imamo u inline_map
    let inline_hashes: Vec<String> = inline_map.values().filter_map(|v| v.clone()).collect();
    let inline_statuses =
        fetch_verification_statuses_pending_or_success(db_pool, &inline_hashes).await?;
    let inline_status_map: HashMap<_, _> = inline_statuses.iter().cloned().collect();

    for class_hash in class_hashes {
        let cairo_version = match class_hash_to_version.get(class_hash) {
            Some(v) => *v,
            None => continue,
        };

        if is_new_cairo_version_supported(cairo_version) {
            //Check for the inline class_hash
            if let Some(Some(inline_class_hash)) = inline_map.get(class_hash) {
                let status = status_map.get(class_hash);
                let inline_status = inline_status_map.get(inline_class_hash);
                match (status, inline_status) {
                    (Some(EVerificationStatus::Success), Some(EVerificationStatus::Success)) => {
                        if let Some(entry) = class_status_map.get(class_hash) {
                            let class_name = entry.0.clone();

                            if let Some(entry_mut) = class_status_map.get_mut(class_hash) {
                                entry_mut.1 = EVerificationStatus::Success;
                                entry_mut.2 = Some("This class is already verified.".to_string());
                            }

                            //                            class_status_map
                            //                                .entry(inline_class_hash.clone())
                            //                                .or_insert_with(|| {
                            //                                    (
                            //                                        class_name,
                            //                                        EVerificationStatus::Success,
                            //                                        Some("This class is already verified.".to_string()),
                            //                                    )
                            //                                });
                        }
                    }
                    _ => {
                        if let Some(entry) = class_status_map.get(class_hash) {
                            let class_name = entry.0.clone();

                            if let Some(entry_mut) = class_status_map.get_mut(class_hash) {
                                entry_mut.1 = EVerificationStatus::Pending;
                                entry_mut.2 = None;
                            }

                            //                            class_status_map.insert(
                            //                                inline_class_hash.clone(),
                            //                                (class_name, EVerificationStatus::Pending, None),
                            //                            );
                        }
                    }
                }
            } else if let Some(entry) = class_status_map.get_mut(class_hash) {
                entry.1 = EVerificationStatus::Pending;
                entry.2 = None;
            }
        } else {
            match status_map.get(class_hash) {
                Some(EVerificationStatus::Pending) => {
                    if let Some(entry) = class_status_map.get_mut(class_hash) {
                        entry.1 = EVerificationStatus::Failed;
                        entry.2 =
                            Some("Verification is already in progress for this class.".to_string());
                    }
                }
                Some(EVerificationStatus::Success) => {
                    if let Some(entry) = class_status_map.get_mut(class_hash) {
                        entry.1 = EVerificationStatus::Success;
                        entry.2 = Some("This class is already verified.".to_string());
                    }
                }
                _ => {}
            }
        }
    }

    Ok(())
}

fn aggregate_statuses(statuses: &[(String, EVerificationStatus)]) -> Option<EVerificationStatus> {
    if statuses
        .iter()
        .any(|(_, s)| *s == EVerificationStatus::Success)
    {
        Some(EVerificationStatus::Success)
    } else if statuses
        .iter()
        .any(|(_, s)| *s == EVerificationStatus::Pending)
    {
        Some(EVerificationStatus::Pending)
    } else {
        None
    }
}

pub async fn update_status_from_verification_table_(
    db_pool: &Pool<Postgres>,
    class_hashes: &[String],
    class_status_map: &mut HashMap<String, (String, EVerificationStatus, Option<String>)>,
) -> Result<()> {
    let statuses = fetch_verification_statuses_pending_or_success(db_pool, class_hashes).await?;
    for (class_hash, status) in statuses {
        match status {
            EVerificationStatus::Pending => {
                if let Some(entry) = class_status_map.get_mut(&class_hash) {
                    entry.1 = EVerificationStatus::Failed;
                    entry.2 =
                        Some("Verification is already in progress for this class.".to_string());
                }
            }
            EVerificationStatus::Success => {
                if let Some(entry) = class_status_map.get_mut(&class_hash) {
                    entry.1 = EVerificationStatus::Success;
                    entry.2 = Some("This class is already verified.".to_string());
                }
            }
            EVerificationStatus::Failed => {
                // Do nothing for failed classes
            }
        }
    }
    Ok(())
}

pub async fn update_status_from_verified_contract_classes_with_inline_class_hash(
    db_pool: &Pool<Postgres>,
    class_hashes: &[String],
    class_hash_to_version: &HashMap<String, (u32, u32, u32)>,
    inline_map: &HashMap<String, Option<String>>,
    class_status_map: &mut HashMap<String, (String, EVerificationStatus, Option<String>)>,
) -> Result<()> {
    let verified_contract_classes = fetch_verified_classes(db_pool, class_hashes).await?;
    let verified_set: HashSet<_> = verified_contract_classes
        .into_iter()
        .map(|c| c.hash)
        .collect();

    for class_hash in class_hashes {
        let is_verified = verified_set.contains(class_hash);
        let version = match class_hash_to_version.get(class_hash) {
            Some(v) => *v,
            None => continue,
        };

        let inline_class_hash_opt = inline_map.get(class_hash).cloned().flatten();

        match inline_class_hash_opt {
            Some(inline_class_hash) if is_new_cairo_version_supported(version) => {
                let inline_is_verified = fetch_verified_class(db_pool, &inline_class_hash)
                    .await
                    .is_ok();

                if is_verified && inline_is_verified {
                    let class_name = class_status_map
                        .get(class_hash)
                        .map(|e| e.0.clone())
                        .unwrap_or_default();

                    if let Some(entry) = class_status_map.get_mut(class_hash) {
                        entry.1 = EVerificationStatus::Success;
                        entry.2 = Some("This class is already verified.".to_string());
                    }

                //                    class_status_map
                //                        .entry(inline_class_hash.clone())
                //                        .or_insert_with(|| {
                //                            (
                //                                class_name,
                //                                EVerificationStatus::Success,
                //                                Some("This class is already verified.".to_string()),
                //                            )
                //                        });
                } else {
                    let class_name = class_status_map
                        .get(class_hash)
                        .map(|e| e.0.clone())
                        .unwrap_or_default();
                    if let Some(entry) = class_status_map.get_mut(class_hash) {
                        entry.1 = EVerificationStatus::Pending;
                        entry.2 = None;
                    }

                    //                    class_status_map.insert(
                    //                        inline_class_hash.clone(),
                    //                        (class_name, EVerificationStatus::Pending, None),
                    //                    );
                }
            }
            _ => {
                if is_verified {
                    if let Some(entry) = class_status_map.get_mut(class_hash) {
                        entry.1 = EVerificationStatus::Success;
                        entry.2 = Some("This class is already verified.".to_string());
                    }
                }
            }
        }
    }
    Ok(())
}

pub async fn update_status_from_verified_contract_classes_(
    db_pool: &Pool<Postgres>,
    class_hashes: &[String],
    class_status_map: &mut HashMap<String, (String, EVerificationStatus, Option<String>)>,
) -> Result<()> {
    let verified_contract_classes = fetch_verified_classes(db_pool, class_hashes).await?;

    for contract_class in verified_contract_classes {
        if let Some(entry) = class_status_map.get_mut(&contract_class.hash) {
            entry.1 = EVerificationStatus::Success;
            entry.2 = Some("This class is already verified.".to_string());
        }
    }

    Ok(())
}

pub async fn fetch_class_from_blockchain(
    provider_client: &JsonRpcClient<HttpTransport>,
    class_hash: &str,
) -> Result<(Vec<Felt>, (u32, u32, u32))> {
    let class_hash_felt = Felt::from_str(class_hash).context("Invalid class hash format")?;

    let class_from_blockchain = provider_client
        .get_class(BlockId::Tag(BlockTag::Latest), class_hash_felt)
        .await
        .context("Failed to get class from the network")?;

    let class_json = serde_json::to_value(&class_from_blockchain)
        .context("Failed to serialize class from blockchain to JSON value")?;
    let class_from_blockchain: CoreContractClass = serde_json::from_value(class_json)
        .context("Failed to deserialize class from JSON value back to CoreContractClass")?;

    let program_from_blockchain = match class_from_blockchain {
        CoreContractClass::Sierra(flattened_sierra_class) => {
            if flattened_sierra_class.sierra_program.len() < 6 {
                let err = anyhow::anyhow!(
                    "Program length is less than 6. Found: {}",
                    flattened_sierra_class.sierra_program.len()
                );
                error!("{:?}", err);
                Err(err)
            } else {
                Ok(flattened_sierra_class.sierra_program)
            }
        }
        _ => {
            let err = anyhow::anyhow!("Contract class is not a Sierra class");
            error!("{:?}", err);
            Err(err)
        }
    }?;

    let cairo_version = extract_cairo_version_from_program(&program_from_blockchain)?;

    Ok((program_from_blockchain, cairo_version))
}

pub async fn process_new_cairo_version_verification(
    manifest: &Manifest,
    tmp_dir: &PathBuf,
    db_pool: &Pool<Postgres>,
    verification_id: Uuid,
    class_verification_data: &mut ClassVerificationData,
) -> Result<()> {
    let mut classes_to_verify_map: HashMap<String, (ContractClass, String)> = HashMap::new();
    if let Err(encountered_error) = handle_new_cairo_verision_class_verification_profiles(
        manifest,
        tmp_dir,
        db_pool,
        verification_id,
        &mut classes_to_verify_map,
    )
    .await
    {
        if let Err(move_err) = move_failed_verification_to_failed_tmp(tmp_dir) {
            let err = format!("Failed to move verification to failed tmp: {:?}", move_err);
            error!("{:?}", err);
            return Err(anyhow::anyhow!(err));
        }
        return Err(encountered_error);
    }

    update_new_cairo_version_class_verification_data(
        class_verification_data,
        &classes_to_verify_map,
    );

    Ok(())
}

async fn handle_new_cairo_verision_class_verification_profiles(
    manifest: &Manifest,
    tmp_dir: &PathBuf,
    db_pool: &Pool<Postgres>,
    verification_id: Uuid,
    classes_to_verify_map: &mut HashMap<String, (ContractClass, String)>,
) -> Result<(), anyhow::Error> {
    let mut encountered_error: Option<anyhow::Error> = None;
    let mut inline_class_hashes = Vec::new();

    for inline_strategy_profile in manifest.profile_with_inline_strategy.keys() {
        info!(
            verification_id = verification_id.to_string(),
            profile = inline_strategy_profile,
            "Processing inline strategy profile:",
        );

        match build_with_scarb_for_profile(manifest, tmp_dir, inline_strategy_profile) {
            Ok(classes) => {
                for (class_hash, contract_class) in classes {
                    inline_class_hashes.push((class_hash.clone(), contract_class.clone()));
                    if let Err(err) = insert_class_hash_profiles(
                        db_pool,
                        &class_hash,
                        inline_strategy_profile,
                        verification_id,
                        &true,
                        Some(&class_hash),
                    )
                    .await
                    {
                        error!(
                            "Failed to insert inline strategy class hash profile: {:?}",
                            err
                        );
                    }

                    classes_to_verify_map
                        .insert(class_hash.clone(), (contract_class, class_hash.clone()));
                }
            }
            Err(e) => {
                error!("Failed to build inline strategy profile: {:?}", e);
                encountered_error = Some(e);
            }
        }
    }

    if encountered_error.is_none() {
        for profile in manifest
            .profiles
            .iter()
            .filter(|&p| !manifest.profile_with_inline_strategy.contains_key(p))
        {
            info!(
                verification_id = verification_id.to_string(),
                profile = profile,
                "Processing profile",
            );
            match build_with_scarb_for_profile(manifest, tmp_dir, profile) {
                Ok(classes) => {
                    if classes.len() == inline_class_hashes.len() {
                        for (idx, (class_hash, contract_class)) in classes.into_iter().enumerate() {
                            if let Some((inline_class_hash, _inline_contract_class)) =
                                inline_class_hashes.get(idx).cloned()
                            {
                                if let Err(err) = insert_class_hash_profiles(
                                    db_pool,
                                    &class_hash,
                                    profile,
                                    verification_id,
                                    &false,
                                    Some(&inline_class_hash),
                                )
                                .await
                                {
                                    error!("Failed to insert class hash with profile: {:?}", err);
                                }

                                classes_to_verify_map
                                    .entry(class_hash)
                                    .or_insert((contract_class, inline_class_hash));
                            }
                        }
                    }
                }
                Err(e) => {
                    error!("Failed to build project profile: {:?}", e);
                    encountered_error = Some(e);
                    break;
                }
            }
        }
    }

    if let Some(e) = encountered_error {
        return Err(e);
    }

    Ok(())
}

fn update_new_cairo_version_class_verification_data(
    class_verification_data: &mut ClassVerificationData,
    classes_to_verify_map: &HashMap<String, (ContractClass, String)>,
) {
    let mut inline_entries_to_add = Vec::new();
    let mut seen_hashes = HashSet::new();

    let mut inline_to_add_candidates = Vec::new();

    for (class_hash, class_result) in class_verification_data.iter_mut() {
        seen_hashes.insert(class_hash.clone());

        match classes_to_verify_map.get(class_hash) {
            Some((contract_class_to_verify, inline_class_hash)) => {
                seen_hashes.insert(inline_class_hash.clone());

                if let Ok((
                    _class_name,
                    _program,
                    _version,
                    ref mut contract_class,
                    ref mut inline_strategy_class_hash,
                    _,
                    _,
                )) = class_result
                {
                    *contract_class = Some(contract_class_to_verify.clone());
                    *inline_strategy_class_hash = Some(inline_class_hash.clone());
                }

                inline_to_add_candidates.push((class_hash.clone(), inline_class_hash.clone()));
            }
            None => {
                let error_message = format!(
                    "Contract class hash does not match. Expected {}, but found {:?}",
                    class_hash,
                    &classes_to_verify_map.keys()
                );
                error!(error_message);
                *class_result = Err(anyhow::anyhow!(error_message));
            }
        }
    }

    for (parent_class_hash, inline_class_hash) in inline_to_add_candidates {
        if !class_verification_data.contains_key(&inline_class_hash) {
            if let Some((inline_contract_class, _)) = classes_to_verify_map.get(&inline_class_hash)
            {
                let new_entry = Ok((
                    parent_class_hash,
                    vec![],
                    (0, 0, 0),
                    Some(inline_contract_class.clone()),
                    Some(inline_class_hash.clone()),
                    None,
                    None,
                ));
                inline_entries_to_add.push((inline_class_hash.clone(), new_entry));
            } else {
                let msg = format!(
                    "Inline contract class {} (from {}) not found in classes_to_verify_map.",
                    inline_class_hash, parent_class_hash
                );
                error!(msg);
            }
        }
    }

    for (inline_class_hash, new_entry) in inline_entries_to_add {
        class_verification_data.insert(inline_class_hash, new_entry);
    }
}

pub async fn process_old_cairo_version_verification(
    manifest: &Manifest,
    cairo_version: (u32, u32, u32),
    tmp_dir: &PathBuf,
    db_pool: &Pool<Postgres>,
    verification_id: Uuid,
    class_verification_data: &mut ClassVerificationData,
) -> Result<()> {
    let mut classes_to_verify_map: HashMap<String, (ContractClass, PathBuf)> = HashMap::new();

    if let Err(encountered_error) = handle_old_cairo_verision_class_verification_profiles(
        manifest,
        cairo_version,
        tmp_dir,
        db_pool,
        verification_id,
        &mut classes_to_verify_map,
    )
    .await
    {
        if let Err(move_err) = move_failed_verification_to_failed_tmp(tmp_dir) {
            let err = format!("Failed to move verification to failed tmp: {:?}", move_err);
            error!("{:?}", err);
            return Err(anyhow::anyhow!(err));
        }
        return Err(encountered_error);
    }

    update_old_cairo_version_class_verification_data(
        class_verification_data,
        &classes_to_verify_map,
    );

    Ok(())
}

async fn handle_old_cairo_verision_class_verification_profiles(
    manifest: &Manifest,
    cairo_version: (u32, u32, u32),
    tmp_dir: &PathBuf,
    db_pool: &Pool<Postgres>,
    verification_id: Uuid,
    classes_to_verify_map: &mut HashMap<String, (ContractClass, PathBuf)>,
) -> Result<(), anyhow::Error> {
    let mut encountered_error: Option<anyhow::Error> = None;

    for profile in &manifest.profiles {
        info!(
            verification_id = verification_id.to_string(),
            profile = profile,
            "Processing profile",
        );
        match compile_with_scarb_for_profile(manifest, cairo_version, tmp_dir, profile) {
            Ok(classes) => {
                for (class_hash, contract_class, cairo_debug_info_path) in classes {
                    if let Err(err) = insert_class_hash_profiles(
                        db_pool,
                        &class_hash,
                        profile,
                        verification_id,
                        &false,
                        None,
                    )
                    .await
                    {
                        error!("Failed to insert class hash with profile: {:?}", err);
                    }

                    classes_to_verify_map
                        .entry(class_hash)
                        .or_insert((contract_class, cairo_debug_info_path));
                }
            }
            Err(e) => {
                encountered_error = Some(e);
                break;
            }
        }
    }

    if let Some(e) = encountered_error {
        return Err(e);
    }

    Ok(())
}

fn update_old_cairo_version_class_verification_data(
    class_verification_data: &mut ClassVerificationData,
    classes_to_verify_map: &HashMap<String, (ContractClass, PathBuf)>,
) {
    for (class_hash, class_result) in class_verification_data.iter_mut() {
        if let Some((contract_class, cairo_debug_info_path)) = classes_to_verify_map.get(class_hash)
        {
            // If the class is found, update the contract_class and cairo_info_path in class_result
            if let Ok((
                _,
                program_from_blockchain,
                _,
                ref mut existing_contract_class,
                _,
                ref mut existing_cairo_debug_info_path,
                ref mut existing_cairo_debug_info,
            )) = class_result
            {
                *existing_contract_class = Some(contract_class.clone());
                *existing_cairo_debug_info_path = Some(cairo_debug_info_path.clone());

                if !programs_match(contract_class, program_from_blockchain) {
                    let err = anyhow::anyhow!(
                        "Contract class does not match for class hash: {}",
                        class_hash
                    );
                    error!("{:?}", err);
                    *class_result = Err(err);
                    continue;
                }

                match load_cairo_debug_info(cairo_debug_info_path) {
                    Ok(debug_info) => *existing_cairo_debug_info = Some(debug_info),
                    Err(err) => {
                        error!("{}", err);
                        *class_result = Err(err);
                    }
                }
            }
        } else {
            let error_message = format!(
                "Contract class hash {} not found in verified classes",
                class_hash
            );
            error!("{}", error_message);
            *class_result = Err(anyhow::anyhow!(error_message));
        }
    }
}

fn programs_match(class: &ContractClass, program_from_blockchain: &[Felt]) -> bool {
    if class.sierra_program.len() != program_from_blockchain.len() {
        return false;
    }
    class
        .sierra_program
        .iter()
        .skip(6)
        .zip(program_from_blockchain.iter().skip(6))
        .all(|(e1, e2)| e1.value.to_string() == e2.to_string())
}

pub fn load_cairo_debug_info(cairo_debug_info_path: &PathBuf) -> Result<SierraToCairoDebugInfo> {
    let cairo_debug_info_file = File::open(cairo_debug_info_path).map_err(|e| {
        anyhow::anyhow!(
            "Failed to open debug info file {}: {:?}",
            cairo_debug_info_path.display(),
            e
        )
    })?;

    let cairo_debug_info_reader = BufReader::new(cairo_debug_info_file);
    serde_json::from_reader(cairo_debug_info_reader)
        .map_err(|e| anyhow::anyhow!("Failed to deserialize debug info: {:?}", e))
}
