use anyhow::{Context, Result};
use aws_sdk_s3::primitives::ByteStream;
use aws_smithy_types::body::SdkBody;
use cairo_lang_starknet_classes::contract_class::ContractClass;
use sqlx::{Pool, Postgres};
use starknet::core::types::{BlockId, BlockTag, ContractClass as CoreContractClass, FieldElement};
use starknet::providers::jsonrpc::HttpTransport;
use starknet::providers::{JsonRpcClient, Provider};
use starknet_api::core::ChainId;
use std::fs;
use std::io::BufReader;
use std::path::PathBuf;
use std::str::FromStr;
use std::{collections::HashMap, fs::File};
use tracing::error;
use uuid::Uuid;
use walnut_shared::{chain_id_to_readable_string, pad_field_element_to_hex_string_length66};

use crate::db::is_class_verified;
use crate::utils::{create_files_from_map, read_manifest, run_scarb_build};
use crate::{SierraToCairoDebugInfo, VerifiedClassData, SUPPORTED_VERSIONS};

pub async fn verify_by_class_hash(
    db_pool: &Pool<Postgres>,
    s3_client: &aws_sdk_s3::Client,
    provider_client: JsonRpcClient<HttpTransport>,
    class_hash: String,
    class_name: String,
    source_code: HashMap<String, String>,
    chain_id: Option<ChainId>,
    project_id: Option<i32>,
) -> Result<()> {
    let is_verified = is_class_verified(db_pool, class_hash.clone()).await?;
    if is_verified {
        return Err(anyhow::anyhow!("Class is already verified"));
    }

    let random_string = Uuid::new_v4().to_string();
    let mut tmp_dir = PathBuf::from("tmp/verification");
    tmp_dir.push(&random_string);

    let res = verify(
        &tmp_dir,
        provider_client,
        class_hash.clone(),
        class_name,
        &source_code,
    )
    .await;

    fs::remove_dir_all(&tmp_dir)?;

    let (contract_class, cairo_debug_info) = res?;

    let is_cairo_debug_info = cairo_debug_info.is_some();

    let verified_class_data = VerifiedClassData {
        contract_class,
        cairo_debug_info,
        source_code,
    };

    let json_data = serde_json::to_string(&verified_class_data)?;

    s3_client
        .put_object()
        .bucket("walnutserver-east-1-classes-verification")
        .key(format!("class-{}.json", class_hash))
        .body(ByteStream::new(SdkBody::from(json_data)))
        .send()
        .await?;

    let _rec = sqlx::query!(
        r#"
    INSERT INTO contract_classes ( hash, is_sierra_debug_info, is_cairo_debug_info, is_source_code, chain_id, project_id )
    VALUES ( $1, $2, $3, $4, $5, $6 )
            "#,
        class_hash,
        true,
        is_cairo_debug_info,
        true,
        chain_id.map_or(None, |id| Some(chain_id_to_readable_string(&id))),
        project_id
    )
    .execute(db_pool)
    .await?;

    Ok(())
}

pub async fn verify_by_contract_address(
    db_pool: &Pool<Postgres>,
    s3_client: &aws_sdk_s3::Client,
    provider_client: JsonRpcClient<HttpTransport>,
    contract_address: String,
    class_name: String,
    source_code: HashMap<String, String>,
    chain_id: Option<ChainId>,
    project_id: Option<i32>,
) -> Result<String> {
    let class_hash = pad_field_element_to_hex_string_length66(
        provider_client
            .get_class_hash_at(
                BlockId::Tag(BlockTag::Latest),
                &FieldElement::from_str(contract_address.as_str())
                    .context("Contract address format is incorrect")?,
            )
            .await
            .context("Can't find the contract class on the network")?,
    );

    verify_by_class_hash(
        db_pool,
        s3_client,
        provider_client,
        class_hash.clone(),
        class_name,
        source_code,
        chain_id,
        project_id,
    )
    .await?;

    Ok(class_hash)
}

async fn verify(
    tmp_dir: &PathBuf,
    provider_client: JsonRpcClient<HttpTransport>,
    class_hash: String,
    class_name: String,
    source_code: &HashMap<String, String>,
) -> Result<(ContractClass, Option<SierraToCairoDebugInfo>)> {
    create_files_from_map(source_code, &tmp_dir)?;

    let scarb_config_file = tmp_dir.join("Scarb.toml");
    let manifest = read_manifest(&scarb_config_file)?;

    let class_from_blockchain = provider_client
        .get_class(
            BlockId::Tag(BlockTag::Latest),
            &FieldElement::from_str(class_hash.as_str())?,
        )
        .await
        .map_err(|e| {
            error!("Failed to get class from the network: {:?}", e);
            e
        })?;

    let program_from_blockchain = match class_from_blockchain {
        CoreContractClass::Sierra(flattened_sierra_class) => {
            Ok(flattened_sierra_class.sierra_program)
        }
        _ => {
            let err = anyhow::anyhow!("Contract class is not a Sierra class");
            error!("{:?}", err);
            Err(err)
        }
    }?;

    // Extract Cairo version from Sierra program
    let starknet_version: (u32, u32, u32) = (
        program_from_blockchain[3].try_into()?,
        program_from_blockchain[4].try_into()?,
        program_from_blockchain[5].try_into()?,
    );

    let is_arm64 = cfg!(target_arch = "aarch64");
    match starknet_version {
        version if SUPPORTED_VERSIONS.contains(&version) => {
            let scarb_path = match version {
                (2, 6, 3) => {
                    if is_arm64 {
                        "scarb/scarb_cairo_v_2_6_3_arm"
                    } else {
                        "scarb/scarb_cairo_v_2_6_3"
                    }
                }
                (2, 6, 4) => {
                    if is_arm64 {
                        "scarb/scarb_cairo_v_2_6_4_arm"
                    } else {
                        "scarb/scarb_cairo_v_2_6_4"
                    }
                }
                (2, 7, 0) => {
                    if is_arm64 {
                        "scarb/scarb_cairo_v_2_7_0_arm"
                    } else {
                        "scarb/scarb_cairo_v_2_7_0"
                    }
                }
                _ => unreachable!(),
            };
            run_scarb_build(&tmp_dir, scarb_path)?;
        }
        _ => {
            error!(
                "Unsupported Cairo version {}.{}.{}",
                starknet_version.0, starknet_version.1, starknet_version.2
            );
            return Err(anyhow::anyhow!("Unsupported Cairo version. Currently, we support versions 2.6.3, 2.6.4, 2.7.0 and will add support for more versions soon. Contact us if you need support for a different version: https://t.me/walnuthq"));
        }
    };

    let contract_class_path = tmp_dir.join("target/dev").join(format!(
        "{}_{}.contract_class.json",
        manifest.package_name, class_name
    ));
    let contract_class_file = File::open(&contract_class_path).map_err(|e| {
        error!("Failed to open contract class file: {:?}", e);
        e
    })?;
    let contract_class_reader = BufReader::new(contract_class_file);
    let contract_class: ContractClass =
        serde_json::from_reader(contract_class_reader).map_err(|e| {
            error!("Failed to deserialize contract class: {:?}", e);
            e
        })?;

    if contract_class.sierra_program.len() != program_from_blockchain.len() {
        let err = anyhow::anyhow!("Contract class does not match");
        error!("Contract class programs lengths don't match");
        return Err(err);
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
            return Err(err);
        }
    }

    let cairo_debug_info: Result<Option<SierraToCairoDebugInfo>> = match starknet_version {
        version if SUPPORTED_VERSIONS.contains(&version) => {
            let cairo_debug_info_path = tmp_dir.join("target/dev").join(format!(
                "{}_{}.contract_class_debug.json",
                manifest.package_name, class_name
            ));
            let cairo_debug_info_file = File::open(&cairo_debug_info_path).map_err(|e| {
                error!(
                    "Failed to open debug info file {}: {:?}",
                    cairo_debug_info_path.display(),
                    e
                );
                e
            })?;
            let cairo_debug_info_reader = BufReader::new(cairo_debug_info_file);
            let cairo_debug_info: SierraToCairoDebugInfo =
                serde_json::from_reader(cairo_debug_info_reader).map_err(|e| {
                    error!("Failed to deserialize debug info: {:?}", e);
                    e
                })?;
            Ok(Some(cairo_debug_info))
        }
        _ => Ok(None),
    };
    let cairo_debug_info = cairo_debug_info.map_err(|e| {
        error!("Failed to process debug info: {:?}", e);
        e
    })?;

    Ok((contract_class, cairo_debug_info))
}
