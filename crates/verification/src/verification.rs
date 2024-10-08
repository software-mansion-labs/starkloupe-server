use anyhow::{Context, Result};
use aws_sdk_s3::primitives::ByteStream;
use aws_smithy_types::body::SdkBody;
use sqlx::{Pool, Postgres};
use starknet::core::types::{ContractClass as CoreContractClass, Felt};
use starknet_old::core::types as starknet_old_types;
use starknet_providers::jsonrpc::HttpTransport;
use starknet_providers::{JsonRpcClient, Provider};
use std::fs;
use std::io::BufReader;
use std::path::PathBuf;
use std::str::FromStr;
use std::{collections::HashMap, fs::File};
use tracing::{error, info};
use uuid::Uuid;
use walnut_shared::{felt_to_field_element, field_element_to_felt};

use crate::scarb::compile_with_scarb;
use crate::sozo::compile_with_sozo;
use crate::utils::{create_files_from_map, read_manifest};
use crate::{ClassVerificationData, EVerificationStatus};
use crate::{SierraToCairoDebugInfo, VerifiedClassData};

pub async fn verify_by_contract_address(
    db_pool: &Pool<Postgres>,
    s3_client: &aws_sdk_s3::Client,
    provider_client: JsonRpcClient<HttpTransport>,
    contract_address: String,
    class_name: String,
    source_code: HashMap<String, String>,
    chain_id: Option<String>,
    project_id: Option<i32>,
) -> Result<Uuid> {
    let class_hash = field_element_to_felt(
        provider_client
            .get_class_hash_at(
                starknet_old_types::BlockId::Tag(starknet_old_types::BlockTag::Latest),
                &felt_to_field_element(
                    Felt::from_str(contract_address.as_str())
                        .context("Contract address format is incorrect")?,
                ),
            )
            .await
            .context("Can't find the contract class on the network")?,
    )
    .to_fixed_hex_string();

    initiate_verification(
        db_pool,
        s3_client,
        provider_client,
        vec![class_hash],
        vec![class_name],
        source_code,
        chain_id,
        project_id,
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
    project_id: Option<i32>,
) -> Result<Uuid> {
    // Create a map to store the status of each class
    // class_hash -> (class_name, status, message)
    let mut class_status_map: HashMap<String, (String, EVerificationStatus, Option<String>)> =
        HashMap::new();

    // Initialize the map with the class hashes and class names
    for (i, class_hash) in class_hashes.iter().enumerate() {
        let class_name = class_names.get(i).unwrap_or(&String::new()).clone();
        let status = EVerificationStatus::Pending;
        class_status_map.insert(class_hash.clone(), (class_name, status, None));
    }

    let results = sqlx::query!(
        r#"
        SELECT class_hash, status as "status: EVerificationStatus"
        FROM verification_status
        WHERE class_hash = ANY($1) AND status IN ('pending', 'success')
        ORDER BY updated_at DESC
        "#,
        &class_hashes
    )
    .fetch_all(db_pool)
    .await?;

    for row in results {
        match row.status {
            EVerificationStatus::Pending => {
                if let Some(class_hash) = row.class_hash {
                    if let Some(entry) = class_status_map.get_mut(&class_hash) {
                        entry.1 = EVerificationStatus::Failed;
                        entry.2 =
                            Some("Verification is already in progress for this class.".to_string());
                    }
                }
            }
            EVerificationStatus::Success => {
                if let Some(class_hash) = row.class_hash {
                    if let Some(entry) = class_status_map.get_mut(&class_hash) {
                        entry.1 = EVerificationStatus::Success;
                        entry.2 = Some("This class is already verified.".to_string());
                    }
                }
            }
            EVerificationStatus::Failed => {
                // Do nothing for failed classes
            }
        }
    }

    let verified_contract_classes = sqlx::query!(
        r#"
        SELECT hash
        FROM contract_classes
        WHERE hash = ANY($1)
        "#,
        &class_hashes
    )
    .fetch_all(db_pool)
    .await?;

    for contract_class in verified_contract_classes {
        if let Some(entry) = class_status_map.get_mut(&contract_class.hash) {
            entry.1 = EVerificationStatus::Success;
            entry.2 = Some("This class is already verified.".to_string());
        }
    }

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
        sqlx::query!(
            r#"
            INSERT INTO verification_status (id, network, class_hash, status, message, created_at, updated_at, project_id)
            VALUES ($1, $2, $3, $4, $5, NOW(), NOW(), $6)
            "#,
            verification_status_id,
            chain_id.clone().unwrap_or_default(),
            class_hash,
            status.to_string(),
            message.clone().unwrap_or_default(),
            project_id
        )
        .execute(db_pool)
        .await
        .context("Failed to insert verification status entry")?;

        match status {
            EVerificationStatus::Pending => {
                info!(
                    class_hash = class_hash,
                    verification_id = verification_status_id.to_string(),
                    project_id = project_id,
                    chain_id = chain_id,
                    tags.verification_status = "pending",
                    "Verification is pending",
                );
            }
            EVerificationStatus::Success => {
                info!(
                    class_hash = class_hash,
                    verification_id = verification_status_id.to_string(),
                    project_id = project_id,
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
                    project_id = project_id,
                    chain_id = chain_id,
                    tags.verification_status = "failed",
                    "Verification failed: {}",
                    message.clone().unwrap_or_default()
                );
            }
        };
    }

    // Sort the classes to verify
    let pending_classes: Vec<(String, String)> = class_status_map
        .iter()
        .filter_map(|(class_hash, (class_name, status, _))| {
            if *status == EVerificationStatus::Pending {
                Some((class_hash.clone(), class_name.clone()))
            } else {
                None
            }
        })
        .collect();

    if pending_classes.is_empty() {
        return Ok(verification_status_id);
    }

    // Make a list of class hashes to verify
    let pending_class_hashes: Vec<String> = pending_classes
        .iter()
        .map(|(class_hash, _)| class_hash.clone())
        .collect();

    let db_pool_clone = db_pool.clone();
    let s3_client_clone = s3_client.clone();
    let chain_id_clone = chain_id.clone();

    tokio::spawn(async move {
        match verify_by_class_hashes(
            &db_pool_clone,
            &s3_client_clone,
            provider_client,
            pending_classes,
            source_code,
            chain_id_clone,
            project_id,
        )
        .await
        {
            Ok(s) => {
                for (class_hash, (status, message)) in &s {
                    let _ = sqlx::query!(
                        r#"
                    UPDATE verification_status
                    SET status = $1, message = $2, updated_at = NOW()
                    WHERE id = $3 AND class_hash = $4
                    "#,
                        status.to_string(),
                        message.clone().unwrap_or_default(),
                        verification_status_id,
                        class_hash
                    )
                    .execute(&db_pool_clone)
                    .await;

                    match status {
                        EVerificationStatus::Pending => {
                            unreachable!();
                        }
                        EVerificationStatus::Success => {
                            info!(
                                class_hash = class_hash,
                                verification_id = verification_status_id.to_string(),
                                project_id = project_id,
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
                                project_id = project_id,
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
                let _ = sqlx::query!(
                    r#"
                UPDATE verification_status
                SET status = 'failed', message = $1, updated_at = NOW()
                WHERE id = $2 AND class_hash = ANY($3)
                "#,
                    e.to_string(),
                    verification_status_id,
                    &pending_class_hashes
                )
                .execute(&db_pool_clone)
                .await;

                for class_hash in &pending_class_hashes {
                    error!(
                        class_hash = class_hash,
                        verification_id = verification_status_id.to_string(),
                        project_id = project_id,
                        chain_id = chain_id,
                        tags.verification_status = "failed",
                        "Verification failed: {}",
                        e.to_string()
                    );
                }
            }
        }
    });

    Ok(verification_status_id)
}

pub async fn verify_by_class_hashes(
    db_pool: &Pool<Postgres>,
    s3_client: &aws_sdk_s3::Client,
    provider_client: JsonRpcClient<HttpTransport>,
    classes: Vec<(String, String)>, // class_hash, class_name
    source_code: HashMap<String, String>,
    chain_id: Option<String>,
    project_id: Option<i32>,
) -> Result<HashMap<String, (EVerificationStatus, Option<String>)>> {
    let random_string = Uuid::new_v4().to_string();
    let mut tmp_dir = PathBuf::from("tmp/verification");
    tmp_dir.push(&random_string);

    let class_verification_data = verify(&tmp_dir, provider_client, classes, &source_code).await;

    fs::remove_dir_all(&tmp_dir)?;

    let class_verification_data: ClassVerificationData = class_verification_data?;

    let mut class_status_map: HashMap<String, (EVerificationStatus, Option<String>)> =
        HashMap::new();

    for (class_hash, class_result) in class_verification_data.iter() {
        if let Ok((_, _, _, Some(contract_class), _, cairo_debug_info)) = class_result {
            let verified_class_data = VerifiedClassData {
                contract_class: contract_class.clone(),
                cairo_debug_info: cairo_debug_info.clone(),
                source_code: source_code.clone(),
            };

            let json_data = serde_json::to_string(&verified_class_data)?;

            s3_client
                .put_object()
                .bucket("walnutserver-east-1-classes-verification")
                .key(format!("class-{}.json", class_hash))
                .body(ByteStream::new(SdkBody::from(json_data)))
                .send()
                .await?;

            let is_cairo_debug_info = cairo_debug_info.is_some();

            let _rec = sqlx::query!(
                r#"
                INSERT INTO contract_classes ( hash, is_sierra_debug_info, is_cairo_debug_info, is_source_code, chain_id, project_id )
                VALUES ( $1, $2, $3, $4, $5, $6 )
                        "#,
                    class_hash,
                    true,
                    is_cairo_debug_info,
                    true,
                    chain_id,
                    project_id
                )
                .execute(db_pool)
                .await?;

            class_status_map.insert(class_hash.clone(), (EVerificationStatus::Success, None));
        } else if let Err(e) = class_result {
            class_status_map.insert(
                class_hash.clone(),
                (EVerificationStatus::Failed, Some(e.to_string())),
            );
        }
    }

    Ok(class_status_map)
}

async fn fetch_class_from_blockchain(
    provider_client: &JsonRpcClient<HttpTransport>,
    class_hash: &str,
) -> Result<(Vec<Felt>, (u32, u32, u32))> {
    let class_from_blockchain = provider_client
        .get_class(
            starknet_old_types::BlockId::Tag(starknet_old_types::BlockTag::Latest),
            &felt_to_field_element(
                Felt::from_str(class_hash).context("Invalid class hash format")?,
            ),
        )
        .await
        .context("Failed to get class from the network")?;

    let class_json = serde_json::to_value(&class_from_blockchain)
        .context("Failed to serialize class from blockchain to JSON value")?;
    let class_from_blockchain: CoreContractClass = serde_json::from_value(class_json)
        .context("Failed to deserialize class from JSON value back to CoreContractClass")?;

    let program_from_blockchain = match &class_from_blockchain {
        CoreContractClass::Sierra(flattened_sierra_class) => {
            Ok(flattened_sierra_class.sierra_program.clone())
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

async fn verify(
    tmp_dir: &PathBuf,
    provider_client: JsonRpcClient<HttpTransport>,
    classes: Vec<(String, String)>, // class_hash, class_name
    source_code: &HashMap<String, String>,
) -> Result<ClassVerificationData> {
    create_files_from_map(source_code, &tmp_dir)?;

    let manifest = read_manifest(&tmp_dir)?;

    let mut class_verification_data: ClassVerificationData = HashMap::new();

    let classes_from_blockchain =
        futures::future::join_all(classes.iter().map(|(class_hash, _class_name)| {
            fetch_class_from_blockchain(&provider_client, class_hash)
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
        let err = anyhow::anyhow!("Failed to fetch classes from the network");
        error!("{:?}", err);
        err
    })?;

    match manifest.has_dojo_target || manifest.dojo_alpha_version.is_some() {
        true => compile_with_sozo(manifest, tmp_dir, &mut class_verification_data)?,
        false => compile_with_scarb(
            cairo_version,
            manifest,
            tmp_dir,
            &mut class_verification_data,
        )?,
    };

    'main_loop: for (class_hash, class_result) in class_verification_data.iter_mut() {
        if let Ok((
            _,
            program_from_blockchain,
            _,
            Some(contract_class),
            cairo_debug_info_path,
            cairo_debug_info,
        )) = class_result
        {
            if contract_class.sierra_program.len() != program_from_blockchain.len() {
                let err = anyhow::anyhow!(
                    "Contract class programs lengths don't match for class hash: {}",
                    class_hash
                );
                error!("{}", err);
                *class_result = Err(err);
                continue;
            }
            for (e1, e2) in contract_class
                .sierra_program
                .iter()
                .skip(6)
                .zip(program_from_blockchain.iter().skip(6))
            {
                if e1.value.to_string() != e2.to_string() {
                    let err = anyhow::anyhow!("Contract class does not match");
                    error!("{:?}", err);
                    *class_result = Err(err);
                    continue 'main_loop;
                }
            }
            if let Some(cairo_debug_info_path) = cairo_debug_info_path {
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
                        continue 'main_loop;
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
                            continue 'main_loop;
                        }
                    };
                *cairo_debug_info = Some(cairo_debug_info_deserialized);
            }
        }
    }

    Ok(class_verification_data)
}
