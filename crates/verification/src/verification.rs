use crate::db::{
    fetch_inline_class_hashes_for_class_hashes, insert_contract_class, insert_verification_request,
    insert_verification_status, update_verification_request, update_verification_status,
    update_verification_statuses,
};
use crate::helpers::{
    fetch_class_from_blockchain, initialize_status_map, process_new_cairo_version_verification,
    process_old_cairo_version_verification,
    update_status_from_verification_table_with_inline_class_hash,
    update_status_from_verified_contract_classes_with_inline_class_hash,
};
use crate::manifest::Manifest;
use crate::s3::upload_class_to_s3;
use crate::scarb::{is_cairo_version_supported, is_new_cairo_version_supported};
use crate::utils::{create_files_from_map, create_temp_directory, remove_walnut_debug_from_scarb};
use crate::{ClassVerificationData, EVerificationStatus};
use anyhow::{Context, Result};
use sqlx::{Pool, Postgres};
use starknet::core::types::{BlockId, BlockTag, Felt};
use starknet::providers::{
    jsonrpc::{HttpTransport, JsonRpcClient},
    Provider,
};
use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;
use std::str::FromStr;
use tracing::{error, info};
use uuid::Uuid;
use walnut_shared::tuple_to_version_string;

pub async fn verify_by_class_hash(
    db_pool: &Pool<Postgres>,
    s3_client: &aws_sdk_s3::Client,
    provider_client: JsonRpcClient<HttpTransport>,
    class_hash: String,
    class_name: String,
    source_code: HashMap<String, String>,
    chain_id: Option<String>,
) -> Result<Uuid> {
    initiate_verification(
        db_pool,
        s3_client,
        provider_client,
        vec![class_hash],
        vec![class_name],
        source_code,
        chain_id,
    )
    .await
}

pub async fn verify_by_contract_address(
    db_pool: &Pool<Postgres>,
    s3_client: &aws_sdk_s3::Client,
    provider_client: JsonRpcClient<HttpTransport>,
    contract_address: String,
    class_name: String,
    source_code: HashMap<String, String>,
    chain_id: Option<String>,
) -> Result<Uuid> {
    let contract_address_felt =
        Felt::from_str(&contract_address).context("Contract address format is incorrect")?;
    let class_hash_felt = provider_client
        .get_class_hash_at(BlockId::Tag(BlockTag::Latest), contract_address_felt)
        .await
        .context("Can't find the contract class on the network")?;

    let class_hash = class_hash_felt.to_fixed_hex_string();

    initiate_verification(
        db_pool,
        s3_client,
        provider_client,
        vec![class_hash],
        vec![class_name],
        source_code,
        chain_id,
    )
    .await
}

pub async fn initiate_verification(
    db_pool: &Pool<Postgres>,
    s3_client: &aws_sdk_s3::Client,
    provider_client: JsonRpcClient<HttpTransport>,
    class_hashes: Vec<String>,
    class_names: Vec<String>,
    source_code: HashMap<String, String>,
    chain_id: Option<String>,
) -> Result<Uuid> {
    let network = chain_id.as_deref().unwrap_or("unknown");
    let classes_from_blockchain = futures::future::join_all(
        class_hashes
            .iter()
            .map(|class_hash| fetch_class_from_blockchain(&provider_client, class_hash, network)),
    )
    .await;

    let class_hash_to_cairo_version: HashMap<String, (u32, u32, u32)> = class_hashes
        .iter()
        .zip(classes_from_blockchain.into_iter())
        .filter_map(|(class_hash, result)| {
            match result {
                Ok((_, cairo_version)) => Some((class_hash.to_string(), cairo_version)),
                Err(err) => {
                    // po želji: loguj grešku
                    error!("Failed to fetch class for hash {}: {:?}", class_hash, err);
                    None
                }
            }
        })
        .collect();
    // Initializes a map with class hashes and their statuses.
    // class_hash -> (class_name, status, message)
    let mut class_status_map = initialize_status_map(&class_hashes, &class_names);

    let class_hash_to_inline_class_hash_map =
        fetch_inline_class_hashes_for_class_hashes(db_pool, &class_hashes).await?;

    update_status_from_verification_table_with_inline_class_hash(
        db_pool,
        &class_hashes,
        &class_hash_to_cairo_version,
        &class_hash_to_inline_class_hash_map,
        &mut class_status_map,
    )
    .await?;

    update_status_from_verified_contract_classes_with_inline_class_hash(
        db_pool,
        &class_hashes,
        &class_hash_to_cairo_version,
        &class_hash_to_inline_class_hash_map,
        &mut class_status_map,
    )
    .await?;

    //update_status_from_verification_table(db_pool, &class_hashes, &mut class_status_map).await?;
    //update_status_from_verified_contract_classes(db_pool, &class_hashes, &mut class_status_map).await?;

    // If there is only one class to verify, check if we already have a status for it
    if class_status_map.len() == 1 {
        let (class_hash, (_, status, message)) = class_status_map.iter().next().unwrap();
        if *status == EVerificationStatus::Failed || *status == EVerificationStatus::Success {
            return Err(anyhow::anyhow!(
                "Class {} verification cannot proceed: {}",
                class_hash,
                message
                    .clone()
                    .unwrap_or_else(|| "Unknown error".to_string())
            ));
        }
    }

    let verification_status_id = Uuid::new_v4();

    // Insert the initial verification status for each class
    for (class_hash, (_, status, message)) in &class_status_map {
        if let Err(e) = insert_verification_status(
            db_pool,
            verification_status_id,
            class_hash,
            status.as_str(),
            message.as_deref(),
            chain_id.as_deref(),
        )
        .await
        {
            error!("Failed to insert verification status entry: {:?}", e);
        }

        match status {
            EVerificationStatus::Pending => {
                info!(
                    class_hash = class_hash,
                    verification_id = verification_status_id.to_string(),
                    chain_id = chain_id,
                    tags.verification_status = "pending",
                    "Verification is pending",
                );
            }
            EVerificationStatus::Success => {
                info!(
                    class_hash = class_hash,
                    verification_id = verification_status_id.to_string(),
                    chain_id = chain_id,
                    tags.verification_status = "success",
                    message = message,
                    "Verification succeeded",
                );
            }
            EVerificationStatus::Failed => {
                error!(
                    class_hash = class_hash,
                    verification_id = verification_status_id.to_string(),
                    chain_id = chain_id,
                    tags.verification_status = "failed",
                    "Verification failed: {}",
                    message.clone().unwrap_or_default()
                );
            }
        };
    }

    let mut pending_class_hashes: Vec<String> = Vec::new();

    let pending_classes: Vec<(String, String)> = class_status_map
        .iter()
        .filter_map(|(class_hash, (class_name, status, _))| {
            if *status == EVerificationStatus::Pending {
                pending_class_hashes.push(class_hash.clone());
                Some((class_hash.clone(), class_name.clone()))
            } else {
                None
            }
        })
        .collect();

    if pending_classes.is_empty() {
        return Ok(verification_status_id);
    }

    let db_pool_clone = db_pool.clone();
    let s3_client_clone = s3_client.clone();
    let chain_id_clone = chain_id.clone();

    tokio::spawn(async move {
        match verify_by_class_hashes(
            &db_pool_clone,
            &s3_client_clone,
            provider_client,
            pending_classes,
            verification_status_id,
            source_code,
            chain_id_clone,
        )
        .await
        {
            Ok(s) => {
                for (class_hash, (status, message)) in &s {
                    if class_status_map.contains_key(class_hash) {
                        if let Err(err) = update_verification_status(
                            &db_pool_clone,
                            verification_status_id,
                            class_hash,
                            status.as_str(),
                            message,
                        )
                        .await
                        {
                            error!("Failed to update verification  status: {:?}", err)
                        };
                    } else {
                        if let Err(e) = insert_verification_status(
                            &db_pool_clone,
                            verification_status_id,
                            class_hash,
                            status.as_str(),
                            message.as_deref(),
                            chain_id.as_deref(),
                        )
                        .await
                        {
                            error!("Failed to insert verification status entry: {:?}", e);
                        }
                    }

                    match status {
                        EVerificationStatus::Pending => {
                            unreachable!();
                        }
                        EVerificationStatus::Success => {
                            info!(
                                class_hash = class_hash,
                                verification_id = verification_status_id.to_string(),
                                chain_id = chain_id,
                                tags.verification_status = "success",
                                message = message,
                                "Verification succeeded",
                            );
                        }
                        EVerificationStatus::Failed => {
                            error!(
                                class_hash = class_hash,
                                verification_id = verification_status_id.to_string(),
                                chain_id = chain_id,
                                tags.verification_status = "failed",
                                "Verification failed: {}",
                                message.clone().unwrap_or_default()
                            );
                        }
                    };
                }
            }
            Err(e) => {
                if let Err(err) = update_verification_statuses(
                    &db_pool_clone,
                    verification_status_id,
                    &pending_class_hashes,
                    EVerificationStatus::Failed.as_str(),
                    &Some(e.to_string()),
                )
                .await
                {
                    error!("Failed to update verification  statuses: {:?}", err)
                };
            }
        }
    });

    Ok(verification_status_id)
}

pub async fn verify_by_class_hashes(
    db_pool: &Pool<Postgres>,
    s3_client: &aws_sdk_s3::Client,
    provider_client: JsonRpcClient<HttpTransport>,
    classes: Vec<(String, String)>,
    verification_id: Uuid,
    mut source_code: HashMap<String, String>,
    chain_id: Option<String>,
) -> Result<HashMap<String, (EVerificationStatus, Option<String>)>> {
    let tmp_dir = create_temp_directory(verification_id.to_string())?;
    let network = chain_id.as_deref().unwrap_or("unknown");

    match verify(
        &tmp_dir,
        db_pool,
        &provider_client,
        classes,
        verification_id,
        &mut source_code,
        network,
    )
    .await
    {
        Ok(class_verification_data) => {
            fs::remove_dir_all(&tmp_dir)?;
            info!(
                verification_id = verification_id.to_string(),
                tags.verification_status = "success",
                "Verification request succeeded; we successfully finished the project build",
            );
            if let Err(e) = update_verification_request(
                db_pool,
                verification_id,
                EVerificationStatus::Success.as_str(),
                &None,
            )
            .await
            {
                error!("Failed to update verification request status: {:?}", e);
            }

            let mut class_status_map: HashMap<String, (EVerificationStatus, Option<String>)> =
                HashMap::new();

            for (class_hash, class_result) in class_verification_data.iter() {
                if let Ok((
                    _,
                    _,
                    _,
                    Some(contract_class),
                    inline_strategy_class_hash,
                    _,
                    cairo_debug_info,
                )) = class_result
                {
                    //We are uploading to s3 the related inline class hash and original class hash if it exist, as this one we need to get the
                    //inline denug information
                    remove_walnut_debug_from_scarb(&mut source_code);

                    let is_cairo_debug_info = cairo_debug_info.is_some();

                    // In db for contract_class table, we are insert the class_hash, as this one is from
                    // user request for verification
                    // We are alos inserting the inline class hash here, as this one will need us to get debug info
                    //
                    // 1. Upload and insert for original class_hash

                    // 2. Upload and insert for inline_strategy_class_hash
                    if let Some(_inline_class_hash) = inline_strategy_class_hash {
                        upload_class_to_s3(
                            s3_client,
                            class_hash,
                            contract_class,
                            cairo_debug_info,
                            &source_code,
                        )
                        .await?;

                        insert_contract_class(
                            db_pool,
                            class_hash,
                            true,
                            is_cairo_debug_info,
                            true,
                            chain_id.as_deref(),
                        )
                        .await?;

                        class_status_map
                            .insert(class_hash.clone(), (EVerificationStatus::Success, None));
                    } else {
                        class_status_map
                            .insert(class_hash.clone(), (EVerificationStatus::Success, None));

                        upload_class_to_s3(
                            s3_client,
                            class_hash,
                            contract_class,
                            cairo_debug_info,
                            &source_code,
                        )
                        .await?;

                        insert_contract_class(
                            db_pool,
                            class_hash,
                            true,
                            is_cairo_debug_info,
                            true,
                            chain_id.as_deref(),
                        )
                        .await?;
                    }
                } else if let Err(e) = class_result {
                    class_status_map.insert(
                        class_hash.clone(),
                        (EVerificationStatus::Failed, Some(e.to_string())),
                    );
                }
            }

            Ok(class_status_map)
        }
        Err(e) => {
            error!(
                verification_id = verification_id.to_string(),
                tags.verification_status = "failed",
                error = e.to_string(),
                "Verification request failed; Project build failed",
            );
            if let Err(err) = update_verification_request(
                db_pool,
                verification_id,
                EVerificationStatus::Failed.as_str(),
                &Some(e.to_string()),
            )
            .await
            {
                error!("Failed to update verification request status: {:?}", err);
            }
            Err(e)
        }
    }
}

async fn verify(
    tmp_dir: &PathBuf,
    db_pool: &Pool<Postgres>,
    provider_client: &JsonRpcClient<HttpTransport>,
    classes: Vec<(String, String)>, // class_hash, class_name
    verification_id: Uuid,
    source_code: &mut HashMap<String, String>,
    network: &str,
) -> Result<ClassVerificationData> {
    let mut class_verification_data: ClassVerificationData = HashMap::new();

    let classes_from_blockchain =
        futures::future::join_all(classes.iter().map(|(class_hash, _)| {
            fetch_class_from_blockchain(provider_client, class_hash, network)
        }))
        .await;

    // Classes should have the same Cairo version
    let mut cairo_version = None;

    for (i, result) in classes_from_blockchain.iter().enumerate() {
        let (class_hash, class_name) = &classes[i];
        match result {
            Ok((program_from_blockchain, version)) => {
                if let Some(existing_version) = cairo_version {
                    if existing_version != *version {
                        let err = anyhow::anyhow!("Mismatch in Starknet versions among classes");
                        error!("{:?}", err);
                        return Err(err);
                    }
                } else {
                    cairo_version = Some(*version);
                }
                class_verification_data.insert(
                    class_hash.clone(),
                    Ok((
                        class_name.clone(),
                        program_from_blockchain.clone(),
                        *version,
                        None,
                        None,
                        None,
                        None,
                    )),
                );
            }
            Err(e) => {
                class_verification_data
                    .insert(class_hash.clone(), Err(anyhow::anyhow!(e.to_string())));
            }
        }
    }

    // If there is no Cairo version, then it means that zero classes were fetched from the network
    let cairo_version = cairo_version.ok_or_else(|| {
        let err = anyhow::anyhow!("Failed to fetch classes from the network {}", network);
        error!("{:?}", err);
        err
    })?;

    let manifest = Manifest::new(source_code, Some(cairo_version))?;
    let package_name = manifest.package_name.clone();
    info!(
        verification_id = verification_id.to_string(),
        project = package_name,
        tags.verification_status = "pending",
        "Verification request is pending; we are starting the project build",
    );

    insert_verification_request(
        db_pool,
        verification_id,
        "pending",
        tuple_to_version_string(manifest.cairo_version).as_str(),
        &manifest.package_name,
    )
    .await
    .context("Failed to insert verification request status entry")?;

    create_files_from_map(source_code, tmp_dir)?;

    if is_new_cairo_version_supported(cairo_version) {
        process_new_cairo_version_verification(
            &manifest,
            tmp_dir,
            db_pool,
            verification_id,
            &mut class_verification_data,
        )
        .await?;
    } else if is_cairo_version_supported(cairo_version) {
        process_old_cairo_version_verification(
            &manifest,
            cairo_version,
            tmp_dir,
            db_pool,
            verification_id,
            &mut class_verification_data,
        )
        .await?;
    } else {
        return Err(anyhow::anyhow!(
            "Unsupported Cairo version {}. Contact us if you need support for a different version: https://t.me/walnuthq",
            tuple_to_version_string(cairo_version)
        ));
    }

    Ok(class_verification_data)
}
