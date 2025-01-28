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
use std::sync::atomic::AtomicBool;
use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::{collections::HashMap, fs::File};
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
    let mut classes_to_verify_map: HashMap<String, ContractClass> = HashMap::new();

    // Create a shared atomic flag to ensure move_failed_verification_to_failed_tmp() is only called once
    let error_handled = Arc::new(AtomicBool::new(false));

    let mut futures: FuturesUnordered<_> = manifest
        .profiles
        .iter()
        .cloned()
        .map(|profile| {
            let tmp_dir_clone = tmp_dir.clone();
            let manifest_clone = manifest.clone();
            let error_handled_clone = error_handled.clone();

            tokio::spawn(async move {
                // Check if an error has already been handled for any profile
                if error_handled_clone.load(Ordering::Relaxed) {
                    error!("Skipping profile build due to earlier scarb failure.");
                    return Err(anyhow::anyhow!("Scarb build failed"));
                }

                // Build with Scarb for the given profile
                match build_with_scarb_for_profile(&manifest_clone, &tmp_dir_clone, &profile) {
                    Ok(classes) => Ok::<_, anyhow::Error>((classes, profile)),
                    Err(e) => {
                        if !error_handled_clone.swap(true, Ordering::Relaxed) {
                            if let Err(move_err) =
                                move_failed_verification_to_failed_tmp(&tmp_dir_clone, &e)
                            {
                                error!("Failed to move verification to failed tmp: {:?}", move_err);
                            }
                        }
                        Err(e)
                    }
                }
            })
        })
        .collect();

    while let Some(result) = futures.next().await {
        match result {
            Ok(Ok((classes, profile))) => {
                for (class_hash, contract_class) in classes {
                    if let Err(err) =
                        insert_class_hash_profiles(db_pool, &class_hash, &profile, verification_id)
                            .await
                    {
                        error!("Failed to insert class hash with profile: {:?}", err);
                    }

                    classes_to_verify_map
                        .entry(class_hash)
                        .or_insert(contract_class);
                }
            }
            Ok(Err(e)) => {
                error!("Error processing profile: {:?}", e);
            }
            Err(e) => {
                error!("Tokio task failed: {:?}", e);
            }
        }
    }

    for (class_hash, class_result) in class_verification_data.iter_mut() {
        if let Some(class) = classes_to_verify_map.get(class_hash) {
            // If the class is found, update the contract_class in class_result
            if let Ok((_, _, _, ref mut contract_class, _, _)) = class_result {
                *contract_class = Some(class.clone());
            }
        } else {
            let error_message = format!(
                "Contract class hash does not match. Contract class hash {} was expected but {:?} are found",
                class_hash, &classes_to_verify_map.keys()
            );
            error!(error_message);
            *class_result = Err(anyhow::anyhow!(error_message));
        }
    }

    Ok(())
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

    // Create a shared atomic flag to ensure move_failed_verification_to_failed_tmp() is only called once
    let error_handled = Arc::new(AtomicBool::new(false));

    let mut futures: FuturesUnordered<_> = manifest
        .profiles
        .iter()
        .cloned()
        .map(|profile| {
            let tmp_dir_clone = tmp_dir.clone();
            let manifest_clone = manifest.clone();
            let error_handled_clone = error_handled.clone();

            tokio::spawn(async move {
                // Check if an error has already been handled for any profile
                if error_handled_clone.load(Ordering::Relaxed) {
                    error!("Skipping profile build due to earlier scarb failure.");
                    return Err(anyhow::anyhow!("Scarb build failed"));
                }

                // Compile with Scarb for the given profile
                match compile_with_scarb_for_profile(
                    &manifest_clone,
                    cairo_version,
                    &tmp_dir_clone,
                    &profile,
                ) {
                    Ok(classes) => Ok::<_, anyhow::Error>((classes, profile)),
                    Err(e) => {
                        if !error_handled_clone.swap(true, Ordering::Relaxed) {
                            if let Err(move_err) =
                                move_failed_verification_to_failed_tmp(&tmp_dir_clone, &e)
                            {
                                error!("Failed to move verification to failed tmp: {:?}", move_err);
                            }
                        }
                        Err(e)
                    }
                }
            })
        })
        .collect();

    while let Some(result) = futures.next().await {
        match result {
            Ok(Ok((classes, profile))) => {
                for (class_hash, contract_class, cairo_debug_info_path) in classes {
                    if let Err(err) =
                        insert_class_hash_profiles(db_pool, &class_hash, &profile, verification_id)
                            .await
                    {
                        error!("Failed to insert class hash with profile: {:?}", err);
                    }

                    classes_to_verify_map
                        .entry(class_hash)
                        .or_insert((contract_class, cairo_debug_info_path));
                }
            }
            Ok(Err(e)) => {
                error!("Error processing profile: {:?}", e);
            }
            Err(e) => {
                error!("Tokio task failed: {:?}", e);
            }
        }
    }

    for (class_hash, class_result) in class_verification_data.iter_mut() {
        if let Some((class, cairo_debug_info_path)) = classes_to_verify_map.get(class_hash) {
            // If the class is found, update the contract_class and cairo_info_path in class_result
            if let Ok((
                _,
                program_from_blockchain,
                _,
                ref mut existing_contract_class,
                ref mut existing_cairo_debug_info_path,
                ref mut existing_cairo_debug_info,
            )) = class_result
            {
                *existing_contract_class = Some(class.clone());
                *existing_cairo_debug_info_path = Some(cairo_debug_info_path.clone());

                if class.sierra_program.len() != program_from_blockchain.len() {
                    let err = anyhow::anyhow!(
                        "Contract class programs lengths don't match for class hash: {}",
                        class_hash
                    );
                    error!("{}", err);
                    *class_result = Err(err);
                    continue;
                }
                let programs_match = class
                    .sierra_program
                    .iter()
                    .skip(6)
                    .zip(program_from_blockchain.iter().skip(6))
                    .all(|(e1, e2)| e1.value.to_string() == e2.to_string());

                if !programs_match {
                    let err = anyhow::anyhow!(
                        "Contract class does not match for class hash: {}",
                        class_hash
                    );
                    error!("{:?}", err);
                    *class_result = Err(err);
                    continue;
                }

                let cairo_debug_info_file = match File::open(&cairo_debug_info_path) {
                    Ok(file) => file,
                    Err(e) => {
                        let err = anyhow::anyhow!(
                            "Failed to open debug info file {}: {:?}",
                            cairo_debug_info_path.display(),
                            e
                        );
                        error!("{}", err);
                        *class_result = Err(err);
                        continue;
                    }
                };
                let cairo_debug_info_reader = BufReader::new(cairo_debug_info_file);
                let cairo_debug_info_deserialized: SierraToCairoDebugInfo =
                    match serde_json::from_reader(cairo_debug_info_reader) {
                        Ok(info) => info,
                        Err(e) => {
                            let err = anyhow::anyhow!("Failed to deserialize debug info: {:?}", e);
                            error!("{}", err);
                            *class_result = Err(err);
                            continue;
                        }
                    };
                *existing_cairo_debug_info = Some(cairo_debug_info_deserialized);
            }
        } else {
            let error_message = format!(
                    "Contract class hash does not match. Contract class hash {} was expected but {:?} are found",
                    class_hash, &classes_to_verify_map.keys()
                );
            error!(error_message);
            *class_result = Err(anyhow::anyhow!(error_message));
        }
    }

    Ok(())
}
