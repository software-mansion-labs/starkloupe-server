use crate::db::{
    fetch_verification_statuses_pending_or_success, fetch_verified_classes,
    insert_class_hash_profiles,
};
use crate::manifest::Manifest;
use crate::scarb::{build_with_scarb_for_profile, compile_with_scarb_for_profile};
use crate::utils::move_failed_verification_to_failed_tmp;
use crate::SierraToCairoDebugInfo;
use crate::{ClassVerificationData, EVerificationStatus};
use anyhow::{Context, Result};
use cairo_lang_starknet_classes::contract_class::ContractClass;
use futures::stream::{FuturesUnordered, StreamExt};
use sqlx::{Pool, Postgres};
use starknet::core::types::{ContractClass as CoreContractClass, Felt};
use starknet_old::core::types::{self as starknet_old_types};
use starknet_providers::jsonrpc::HttpTransport;
use starknet_providers::{JsonRpcClient, Provider};
use std::io::BufReader;
use std::path::PathBuf;
use std::str::FromStr;
use std::{collections::HashMap, fs::File};
use tokio::sync::broadcast::Sender;
use tracing::error;
use uuid::Uuid;
use walnut_shared::felt_to_field_element;

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

pub async fn update_status_from_verification_table(
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

pub async fn update_status_from_verified_contract_classes(
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
    let class_hash_field = felt_to_field_element(class_hash_felt);

    let class_from_blockchain = provider_client
        .get_class(
            starknet_old_types::BlockId::Tag(starknet_old_types::BlockTag::Latest),
            class_hash_field,
        )
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

    let cairo_version: (u32, u32, u32) = (
        program_from_blockchain[3].to_biguint().try_into()?,
        program_from_blockchain[4].to_biguint().try_into()?,
        program_from_blockchain[5].to_biguint().try_into()?,
    );

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
    let encountered_error = spawn_new_cairo_version_verification_tasks(
        manifest,
        tmp_dir,
        db_pool,
        verification_id,
        &mut classes_to_verify_map,
    )
    .await;

    if encountered_error {
        if let Err(move_err) = move_failed_verification_to_failed_tmp(tmp_dir, &verification_id) {
            let err = format!("Failed to move verification to failed tmp: {:?}", move_err);
            error!("{:?}", err);
            return Err(anyhow::anyhow!(err));
        }
        return Err(anyhow::anyhow!(
            "Verification failed due to build project errors."
        ));
    }

    update_new_cairo_version_class_verification_data(
        class_verification_data,
        &classes_to_verify_map,
    );

    Ok(())
}

async fn spawn_new_cairo_version_verification_tasks(
    manifest: &Manifest,
    tmp_dir: &PathBuf,
    db_pool: &Pool<Postgres>,
    verification_id: Uuid,
    classes_to_verify_map: &mut HashMap<String, (ContractClass, String)>,
) -> bool {
    // Broadcast channel for error signalization
    let (tx, _) = tokio::sync::broadcast::channel(1);
    let mut encountered_error = false;
    let mut inline_class_hashes: Vec<(String, ContractClass)> = Vec::new();

    // First build profil with inline strategy, if it exists
    if let Some(inline_strategy_profile) = manifest.profile_with_inline_strategy.keys().next() {
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
                        encountered_error = true;
                    }
                    classes_to_verify_map
                        .insert(class_hash.clone(), (contract_class, class_hash.clone()));
                }
            }
            Err(e) => {
                error!("Failed to build inline strategy profile: {:?}", e);
                return true;
            }
        }
    }

    // Buld other profiles, skip inline profile
    let mut futures: FuturesUnordered<_> = manifest
        .profiles
        .iter()
        .filter(|&profile| !manifest.profile_with_inline_strategy.contains_key(profile))
        .cloned()
        .map(|profile| {
            let tmp_dir_clone = tmp_dir.clone();
            let manifest_clone = manifest.clone();
            let tx = tx.clone();
            // Each thread has its own copy of receiver
            let mut rx = tx.subscribe();

            tokio::spawn(async move {
                tokio::select! {
                    //First check if signal for termination is received
                    biased;
                    _ = rx.recv() => {
                        error!("Skipping profile build due to earlier failure." );
                        Err(anyhow::anyhow!("Build project failed."))
                    }
                result = async {
                    match build_with_scarb_for_profile(&manifest_clone, &tmp_dir_clone, &profile) {
                        Ok(classes) => Ok::<_, anyhow::Error>((classes, profile.clone())),
                        Err(e) => {
                            // Send signal to other thread to terminate
                            let _ = tx.send(());
                            Err(e)
                        }
                    }
                        } => result
                }
            })
        })
        .collect();

    while let Some(result) = futures.next().await {
        match result {
            Ok(Ok((classes, profile))) => {
                if classes.len() == inline_class_hashes.len() {
                    for (idx, (class_hash, _contract_class)) in classes.into_iter().enumerate() {
                        if let Some((inline_class_hash, inline_contract_class)) =
                            inline_class_hashes.get(idx).cloned()
                        {
                            if let Err(err) = insert_class_hash_profiles(
                                db_pool,
                                &class_hash,
                                &profile,
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
                                .or_insert((inline_contract_class, inline_class_hash));
                        }
                    }
                }
            }
            Ok(Err(_e)) => {
                encountered_error = true;
            }
            Err(e) => {
                error!("Tokio task failed: {:?}", e);
                encountered_error = true;
            }
        }
    }

    encountered_error
}

fn update_new_cairo_version_class_verification_data(
    class_verification_data: &mut ClassVerificationData,
    classes_to_verify_map: &HashMap<String, (ContractClass, String)>,
) {
    for (class_hash, class_result) in class_verification_data.iter_mut() {
        if let Some((inline_contract_class, inline_class_hash)) =
            classes_to_verify_map.get(class_hash)
        {
            if let Ok((_, _, _, ref mut contract_class, ref mut inline_strategy_class_hash, _, _)) =
                class_result
            {
                *contract_class = Some(inline_contract_class.clone());
                *inline_strategy_class_hash = Some(inline_class_hash.clone());
            }
        } else {
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

pub async fn process_old_cairo_version_verification(
    manifest: &Manifest,
    cairo_version: (u32, u32, u32),
    tmp_dir: &PathBuf,
    db_pool: &Pool<Postgres>,
    verification_id: Uuid,
    class_verification_data: &mut ClassVerificationData,
) -> Result<()> {
    let mut classes_to_verify_map: HashMap<String, (ContractClass, PathBuf)> = HashMap::new();
    let encountered_error = spawn_old_cairo_version_verification_tasks(
        manifest,
        cairo_version,
        tmp_dir,
        db_pool,
        verification_id,
        &mut classes_to_verify_map,
    )
    .await;

    if encountered_error {
        if let Err(move_err) = move_failed_verification_to_failed_tmp(tmp_dir, &verification_id) {
            let err = format!("Failed to move verification to failed tmp: {:?}", move_err);
            error!("{:?}", err);
            return Err(anyhow::anyhow!(err));
        }
        return Err(anyhow::anyhow!(
            "Verification failed due to project build errors."
        ));
    }

    update_old_cairo_version_class_verification_data(
        class_verification_data,
        &classes_to_verify_map,
    );

    Ok(())
}

async fn spawn_old_cairo_version_verification_tasks(
    manifest: &Manifest,
    cairo_version: (u32, u32, u32),
    tmp_dir: &PathBuf,
    db_pool: &Pool<Postgres>,
    verification_id: Uuid,
    classes_to_verify_map: &mut HashMap<String, (ContractClass, PathBuf)>,
) -> bool {
    // Broadcast channel for error signalization
    let (tx, _) = tokio::sync::broadcast::channel(1);
    let mut encountered_error = false;

    // Buld profiles, skip inline profile
    let mut futures: FuturesUnordered<_> = manifest
        .profiles
        .iter()
        .cloned()
        .map(|profile| {
            let tmp_dir_clone = tmp_dir.clone();
            let manifest_clone = manifest.clone();
            let tx: Sender<()> = tx.clone();
            // Each thread has its own copy of receiver
            let mut rx = tx.subscribe();

            tokio::spawn(async move {
                tokio::select! {
                biased;
                    _ = rx.recv() => {
                        error!("Skipping profile build due to earlier failure.");
                        Err(anyhow::anyhow!("Scarb build failed"))
                    }
                // Compile with Scarb for the given profile
                result = async {
                    match compile_with_scarb_for_profile(
                        &manifest_clone,
                        cairo_version,
                        &tmp_dir_clone,
                        &profile,
                    ) {
                        Ok(classes) => Ok((classes, profile)),
                        Err(e) => {
                            let _ = tx.send(());
                            Err(e)
                        }
                    }
                        } => result
                }
            })
        })
        .collect();

    while let Some(result) = futures.next().await {
        match result {
            Ok(Ok((classes, profile))) => {
                for (class_hash, contract_class, cairo_debug_info_path) in classes {
                    if let Err(err) = insert_class_hash_profiles(
                        db_pool,
                        &class_hash,
                        &profile,
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
            Ok(Err(_e)) => {
                encountered_error = true;
            }
            Err(e) => {
                error!("Tokio task failed: {:?}", e);
                encountered_error = true;
            }
        }
    }

    encountered_error
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
