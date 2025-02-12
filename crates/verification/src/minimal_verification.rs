use anyhow::{Context, Result};
use cairo_lang_starknet_classes::contract_class::ContractClass;
use sqlx::{Pool, Postgres};
use std::collections::{HashMap, HashSet};
use std::fs;
use tracing::error;
use uuid::Uuid;
use walnut_shared::tuple_to_version_string;

use crate::db::{
    fetch_verified_classes, insert_class_hash_profiles, insert_contract_class,
    insert_verification_request, insert_verification_status, update_verification_request,
};
use crate::manifest::Manifest;
use crate::s3::upload_class_to_s3;
use crate::scarb::build_with_scarb_for_profile;
use crate::utils::{
    create_files_from_map, create_temp_directory, move_failed_verification_to_failed_tmp,
    remove_walnut_debug_from_scarb,
};
use crate::EVerificationStatus;
use futures::stream::{FuturesUnordered, StreamExt};

pub async fn initiate_minimal_verification(
    db_pool: &Pool<Postgres>,
    s3_client: &aws_sdk_s3::Client,
    source_code: HashMap<String, String>,
    manifest: Manifest,
) -> Result<Uuid> {
    let verification_request_id = Uuid::new_v4();

    insert_verification_request(
        db_pool,
        verification_request_id,
        "pending",
        tuple_to_version_string(manifest.cairo_version).as_str(),
        &manifest.package_name,
    )
    .await
    .context("Failed to insert verification request status entry")?;

    let db_pool_clone = db_pool.clone();
    let s3_client_clone = s3_client.clone();

    tokio::spawn(async move {
        match verify(
            &db_pool_clone,
            &s3_client_clone,
            verification_request_id,
            source_code,
            manifest,
        )
        .await
        {
            Ok((verified_now_hashes, verified_before_hashes)) => {
                if let Err(e) = update_verification_request(
                    &db_pool_clone,
                    verification_request_id,
                    EVerificationStatus::Success.as_str(),
                    &None,
                )
                .await
                {
                    error!("Failed to update verification request status: {:?}", e);
                }

                for class_hash in &verified_now_hashes {
                    if let Err(e) = insert_verification_status(
                        &db_pool_clone,
                        verification_request_id,
                        class_hash,
                        EVerificationStatus::Success.as_str(),
                        None,
                        None,
                    )
                    .await
                    {
                        error!("Failed to insert verification status entry: {:?}", e);
                    }
                }

                for class_hash in &verified_before_hashes {
                    if let Err(e) = insert_verification_status(
                        &db_pool_clone,
                        verification_request_id,
                        class_hash,
                        EVerificationStatus::Success.as_str(),
                        Some("This class is already verified."),
                        None,
                    )
                    .await
                    {
                        error!("Failed to insert verification status entry: {:?}", e);
                    }
                }
            }
            Err(e) => {
                error!("Verification failed: {:?}", e);
                if let Err(err) = update_verification_request(
                    &db_pool_clone,
                    verification_request_id,
                    EVerificationStatus::Failed.as_str(),
                    &Some(e.to_string()),
                )
                .await
                {
                    error!("Failed to update verification request status: {:?}", err);
                }
            }
        }
    });

    Ok(verification_request_id)
}

async fn verify(
    db_pool: &Pool<Postgres>,
    s3_client: &aws_sdk_s3::Client,
    verification_id: Uuid,
    mut source_code: HashMap<String, String>,
    manifest: Manifest,
) -> Result<(HashSet<String>, HashSet<String>)> {
    let tmp_dir = create_temp_directory()?;
    create_files_from_map(&source_code, &tmp_dir)?;

    let mut verified_contract_classes: HashSet<String> = HashSet::new();
    let mut classes_to_verify_map: HashMap<String, (ContractClass, String)> = HashMap::new();

    let mut encountered_error = false;
    let mut inline_class_hashes: Vec<(String, ContractClass)> = Vec::new();

    // First build profil with inline strategy, if it exists
    if let Some(inline_strategy_profile) = manifest.profile_with_inline_strategy.keys().next() {
        match build_with_scarb_for_profile(&manifest, &tmp_dir, inline_strategy_profile) {
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
            }
        }
    }

    // Broadcast channel for error signalization
    let (tx, rx) = tokio::sync::broadcast::channel(1);
    let mut futures: FuturesUnordered<_> = manifest
        .profiles
        .iter()
        .filter(|&profile| !manifest.profile_with_inline_strategy.contains_key(profile))
        .cloned()
        .map(|profile| {
            let db_pool = db_pool.clone();
            let tmp_dir = tmp_dir.clone();
            let manifest = manifest.clone();
            let tx = tx.clone();
            // Each thread has its own copy of receiver
            let mut rx = rx.resubscribe();

            tokio::spawn(async move {
                tokio::select! {
                    //First check if signal for termination is received
                    biased;
                    _ = rx.recv() => {
                        error!("Skipping profile build due to earlier failure." );
                        Err(anyhow::anyhow!("Build project failed."))
                    }
                    result = async {
                        match build_with_scarb_for_profile(&manifest, &tmp_dir, &profile) {
                            Ok(classes) => {
                                let class_hashes: Vec<String> = classes
                                    .iter()
                                    .map(|(class_hash, _)| class_hash.clone())
                                    .collect();

                                let verified_hashes: Vec<String> =
                                    fetch_verified_classes(&db_pool, &class_hashes)
                                        .await?
                                        .into_iter()
                                        .map(|row| row.hash)
                                        .collect();
                                Ok::<_, anyhow::Error>((verified_hashes, classes, profile))
                            }
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
            Ok(Ok((verified_hashes, classes, profile))) => {
                verified_contract_classes.extend(verified_hashes);
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
                        if !verified_contract_classes.contains(&class_hash) {
                            classes_to_verify_map
                                .entry(class_hash)
                                .or_insert((inline_contract_class, inline_class_hash));
                        }
                    }
                }
            }
            Ok(Err(e)) => {
                error!("Error processing profile: {:?}", e);
                encountered_error = true;
            }
            Err(e) => {
                error!("Tokio task failed: {:?}", e);
                encountered_error = true;
            }
        }
    }

    if encountered_error {
        if let Err(move_err) = move_failed_verification_to_failed_tmp(&tmp_dir, &verification_id) {
            let err = format!("Failed to move verification to failed tmp: {:?}", move_err);
            error!("{:?}", err);
            return Err(anyhow::anyhow!(err));
        }
        return Err(anyhow::anyhow!(
            "Verification failed due to build project errors."
        ));
    }
    // Database and S3 operations are now performed after all class_hash values are collected.
    for class_hash in classes_to_verify_map.keys() {
        if let Some((inline_contract_class, inline_class_hash)) =
            classes_to_verify_map.get(class_hash)
        {
            remove_walnut_debug_from_scarb(&mut source_code);
            upload_class_to_s3(
                s3_client,
                inline_class_hash,
                inline_contract_class,
                &None,
                &source_code,
            )
            .await?;
            insert_contract_class(db_pool, class_hash, true, true, true, None).await?;
        };
    }
    fs::remove_dir_all(&tmp_dir)?;
    Ok((
        classes_to_verify_map.keys().cloned().collect(),
        verified_contract_classes,
    ))
}
