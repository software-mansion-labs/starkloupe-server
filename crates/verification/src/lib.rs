pub mod cairo_debug_info;

use anyhow::Result;
use aws_sdk_s3::primitives::ByteStream;
use aws_smithy_types::body::SdkBody;
use cairo_debug_info::SierraToCairoDebugInfo;
use cairo_lang_starknet_classes::contract_class::ContractClass;
use scarb_api::ScarbCommand;
use serde::{Deserialize, Serialize};
use sqlx::{Pool, Postgres};
use starknet::core::types::{BlockId, BlockTag, ContractClass as CoreContractClass, FieldElement};
use starknet::providers::Provider;
use starknet_api::core::ChainId;
use std::fs;
use std::io::{BufReader, Write};
use std::path::{Path, PathBuf};
use std::str::FromStr;
use std::{collections::HashMap, fs::File};
use uuid::Uuid;
use walnut_shared::create_rpc_client;

#[derive(Debug)]
pub struct VerifiedClassRow {
    pub hash: String,
    pub is_sierra_debug_info: bool,
    pub is_cairo_debug_info: bool,
    pub is_source_code: bool,
}

pub fn key_for_class_hash(class_hash: String) -> String {
    format!("class-{}.json", class_hash)
}

pub async fn fetch_verified_classes(
    db_pool: &Pool<Postgres>,
    class_hashes: Vec<String>,
) -> Result<Vec<VerifiedClassRow>> {
    let verified_classes = sqlx::query_as!(
        VerifiedClassRow,
        r#"SELECT *
        FROM contract_classes
        WHERE hash = ANY($1)"#,
        &class_hashes
    )
    .fetch_all(db_pool)
    .await?;

    Ok(verified_classes)
}

#[derive(Serialize, Deserialize)]
pub struct VerifiedClassData {
    pub contract_class: ContractClass,
    pub cairo_debug_info: Option<SierraToCairoDebugInfo>,
    pub source_code: HashMap<String, String>,
}

async fn is_class_verified(db_pool: &Pool<Postgres>, class_hash: String) -> Result<bool> {
    let result = sqlx::query!(
        r#"SELECT EXISTS ( SELECT 1 from contract_classes WHERE hash = $1 ) "#,
        class_hash
    )
    .fetch_one(db_pool)
    .await?;

    if let Some(e) = result.exists {
        return Ok(e);
    };

    Ok(false)
}

fn field_element_to_padded_hex_string(field_element: FieldElement) -> String {
    let mut hex_string = hex::encode(field_element.to_bytes_be());
    while hex_string.len() < 64 {
        hex_string.insert_str(0, "0");
    }
    format!("0x{}", hex_string)
}

pub async fn verify_by_contract_address(
    db_pool: &Pool<Postgres>,
    s3_client: &aws_sdk_s3::Client,
    chain_id: ChainId,
    contract_address: String,
    class_name: String,
    source_code: HashMap<String, String>,
) -> Result<()> {
    let provider_client = create_rpc_client(&chain_id);
    let class_hash = field_element_to_padded_hex_string(
        provider_client
            .get_class_hash_at(
                BlockId::Tag(BlockTag::Latest),
                &FieldElement::from_str(contract_address.as_str())?,
            )
            .await?,
    );

    let is_verified = is_class_verified(db_pool, class_hash.clone()).await?;
    if is_verified {
        return Err(anyhow::anyhow!("Class is already verified"));
    }

    let random_string = Uuid::new_v4().to_string();
    let mut tmp_dir = PathBuf::from("tmp/verification");
    tmp_dir.push(&random_string);

    let res = verify(
        &tmp_dir,
        chain_id,
        contract_address,
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
    INSERT INTO contract_classes ( hash, is_sierra_debug_info, is_cairo_debug_info, is_source_code )
    VALUES ( $1, $2, $3, $4 )
            "#,
        class_hash,
        true,
        is_cairo_debug_info,
        true
    )
    .execute(db_pool)
    .await?;

    Ok(())
}

async fn verify(
    tmp_dir: &PathBuf,
    chain_id: ChainId,
    contract_address: String,
    class_name: String,
    source_code: &HashMap<String, String>,
) -> Result<(ContractClass, Option<SierraToCairoDebugInfo>)> {
    create_files_from_map(source_code, &tmp_dir)?;

    let scarb_config_file = tmp_dir.join("Scarb.toml");
    let manifest = read_manifest(&scarb_config_file)?;

    match manifest.starknet_version {
        (2, 6, _) => {
            let mut cmd = ScarbCommand::new_with_stdio();
            cmd.current_dir(&tmp_dir);
            let relative_path = PathBuf::from("scarb/scarb_v_2_6_3");
            let absolute_path = fs::canonicalize(&relative_path).unwrap();
            cmd.scarb_path(absolute_path);
            cmd.arg("build");
            cmd.run()?;
        }
        _ => {
            return Err(anyhow::anyhow!("Unsupported starknet version"));
        }
    };

    let contract_class_path = tmp_dir.join("target/dev").join(format!(
        "{}_{}.contract_class.json",
        manifest.package_name, class_name
    ));
    let contract_class_file = File::open(contract_class_path)?;
    let contract_class_reader = BufReader::new(contract_class_file);
    let contract_class: ContractClass = serde_json::from_reader(contract_class_reader)?;

    let provider_client = create_rpc_client(&chain_id);
    let class_from_blockchain = provider_client
        .get_class_at(
            BlockId::Tag(BlockTag::Latest),
            &FieldElement::from_str(contract_address.as_str())?,
        )
        .await?;
    let program_from_blockchain = match class_from_blockchain {
        CoreContractClass::Sierra(flattened_sierra_class) => {
            Ok(flattened_sierra_class.sierra_program)
        }
        _ => Err(anyhow::anyhow!("Contract class is not a Sierra class")),
    }?;

    if contract_class.sierra_program.len() != program_from_blockchain.len() {
        return Err(anyhow::anyhow!(
            "Contract class programs lengths don't match"
        ));
    }

    for (e1, e2) in contract_class
        .sierra_program
        .iter()
        .zip(program_from_blockchain.iter())
    {
        if e1.value.to_string() != e2.to_string() {
            return Err(anyhow::anyhow!("Contract class does not match"));
        }
    }

    let cairo_debug_info: Result<Option<SierraToCairoDebugInfo>> = match manifest.starknet_version {
        (2, 6, _) => {
            let cairo_debug_info_path = tmp_dir.join("target/dev").join(format!(
                "{}_{}.contract_class_debug.json",
                manifest.package_name, class_name
            ));
            let cairo_debug_info_file = File::open(cairo_debug_info_path)?;
            let cairo_debug_info_reader = BufReader::new(cairo_debug_info_file);
            let cairo_debug_info: SierraToCairoDebugInfo =
                serde_json::from_reader(cairo_debug_info_reader)?;
            Ok(Some(cairo_debug_info))
        }
        _ => Ok(None),
    };
    let cairo_debug_info = cairo_debug_info?;

    Ok((contract_class, cairo_debug_info))
}

struct Manifest {
    package_name: String,
    starknet_version: (usize, usize, usize),
}

fn read_manifest(path: &Path) -> Result<Manifest> {
    let contents = fs::read_to_string(path)?;

    // Parse the string as TOML
    let toml = contents.parse::<toml::Value>()?;

    // Navigate to the "dependencies" table and get the "starknet" value
    let starknet_version_str = toml
        .get("dependencies")
        .and_then(|deps| deps.get("starknet"))
        .and_then(toml::Value::as_str);

    let starknet_version = match starknet_version_str {
        None => Err(anyhow::anyhow!("No starknet version found")),
        Some(version) => {
            let version_parts: Vec<usize> =
                version.split('.').map(|s| s.parse().unwrap_or(0)).collect();

            if version_parts.len() != 3 {
                Err(anyhow::anyhow!("Version should have 3 parts"))
            } else {
                Ok((version_parts[0], version_parts[1], version_parts[2]))
            }
        }
    }?;

    let package_name = toml
        .get("package")
        .and_then(|p| p.get("name"))
        .and_then(toml::Value::as_str)
        .ok_or(anyhow::anyhow!("No package name found"))?;

    Ok(Manifest {
        package_name: package_name.to_string(),
        starknet_version,
    })
}

pub fn create_files_from_map(
    source_code: &HashMap<String, String>,
    dir_path: &PathBuf,
) -> Result<()> {
    for (path, content) in source_code {
        let mut full_path = dir_path.clone();
        full_path.push(path);

        if let Some(dir) = full_path.parent() {
            fs::create_dir_all(dir)?;
        }
        let mut file = File::create(&full_path)?;
        file.write_all(content.as_bytes())?;
    }
    Ok(())
}
