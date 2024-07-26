pub mod cairo_debug_info;

use anyhow::{Context, Result};
use aws_sdk_s3::primitives::ByteStream;
use aws_smithy_types::body::SdkBody;
use cairo_debug_info::SierraToCairoDebugInfo;
use cairo_lang_starknet_classes::contract_class::ContractClass;
use scarb_api::ScarbCommand;
use serde::{Deserialize, Serialize};
use sqlx::{Pool, Postgres};
use starknet::core::types::{BlockId, BlockTag, ContractClass as CoreContractClass, FieldElement};
use starknet::providers::jsonrpc::HttpTransport;
use starknet::providers::{JsonRpcClient, Provider};
use starknet_api::core::ChainId;
use std::io::{BufReader, Write};
use std::path::{Path, PathBuf};
use std::str::FromStr;
use std::{collections::HashMap, fs::File};
use std::{env, fs};
use uuid::Uuid;
use walnut_shared::{chain_id_to_readable_string, pad_field_element_to_hex_string_length66};

#[derive(Debug)]
pub struct VerifiedClassRow {
    pub hash: String,
    pub is_sierra_debug_info: bool,
    pub is_cairo_debug_info: bool,
    pub is_source_code: bool,
    pub chain_id: Option<String>,
    pub project_id: Option<i32>,
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

pub async fn fetch_verified_class(
    db_pool: &Pool<Postgres>,
    class_hash: String,
) -> Result<VerifiedClassRow> {
    let verified_class = sqlx::query_as!(
        VerifiedClassRow,
        r#"SELECT *
        FROM contract_classes
        WHERE hash = $1"#,
        &class_hash
    )
    .fetch_one(db_pool)
    .await?;

    Ok(verified_class)
}

pub async fn fetch_verified_class_with_data(
    db_pool: &Pool<Postgres>,
    s3_client: &aws_sdk_s3::Client,
    class_hash: String,
) -> Result<(VerifiedClassRow, VerifiedClassData)> {
    let verified_class = fetch_verified_class(db_pool, class_hash.clone()).await?;

    let resp = s3_client
        .get_object()
        .bucket("walnutserver-east-1-classes-verification")
        .key(key_for_class_hash(class_hash))
        .send()
        .await?;

    let body = resp.body.collect().await?;
    let parsed: VerifiedClassData = serde_json::from_slice(&body.into_bytes())?;

    Ok((verified_class, parsed))
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
        chain_id.map_or(None, |id| Some(chain_id_to_readable_string(id))),
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
            .context("Can't find the contract class")?,
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

    match manifest.starknet_version {
        (2, 6, 3) => {
            run_scarb_build(&tmp_dir, "scarb/scarb_cairo_v_2_6_3")?;
        }
        (2, 6, _) => {
            run_scarb_build(&tmp_dir, "scarb/scarb_cairo_v_2_6_4")?;
        }
        _ => {
            return Err(anyhow::anyhow!("Unsupported Starknet version. Currently, we support versions 2.6.* and will add support for more versions soon."));
        }
    };

    let contract_class_path = tmp_dir.join("target/dev").join(format!(
        "{}_{}.contract_class.json",
        manifest.package_name, class_name
    ));
    let contract_class_file = File::open(contract_class_path)?;
    let contract_class_reader = BufReader::new(contract_class_file);
    let contract_class: ContractClass = serde_json::from_reader(contract_class_reader)?;

    let class_from_blockchain = provider_client
        .get_class(
            BlockId::Tag(BlockTag::Latest),
            &FieldElement::from_str(class_hash.as_str())?,
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

fn run_scarb_build(tmp_dir: &PathBuf, scarb_path: &str) -> Result<()> {
    let mut cmd = ScarbCommand::new_with_stdio();
    cmd.current_dir(tmp_dir);
    let absolute_path = fs::canonicalize(scarb_path)?;
    cmd.scarb_path(absolute_path);
    cmd.arg("build");
    let scarb_cache_dir = env::current_dir()?.join(".cache/scarb");
    let scarb_cache_dir_str = scarb_cache_dir
        .to_str()
        .ok_or(anyhow::anyhow!("Failed to convert cache dir to string"))?;
    cmd.env("SCARB_CACHE", scarb_cache_dir_str);
    cmd.run()?;
    Ok(())
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
        None => Err(anyhow::anyhow!("Starknet version not found. Please specify the Starknet version as a dependency in your Scarb.toml file.")),
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
