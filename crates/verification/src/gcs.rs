use crate::db::{fetch_verified_class, fetch_verified_class_with_inlining_class};
use crate::SierraToCairoDebugInfo;
use crate::{VerifiedClassData, VerifiedClassRow};
use anyhow::Result;
use cairo_lang_starknet_classes::contract_class::ContractClass;
use sqlx::{Pool, Postgres};
use std::collections::{HashMap, HashSet};
use tracing::error;

pub fn key_for_class_hash(class_hash: &str) -> String {
    format!("class-{}.json", class_hash)
}

fn classes_bucket() -> String {
    let bucket_name = std::env::var("GCS_CLASSES_BUCKET_NAME")
        .expect("GCS_CLASSES_BUCKET_NAME environment variable must be set");
    format!("projects/_/buckets/{bucket_name}")
}

pub async fn fetch_verified_class_hash_with_contract_class_data(
    db_pool: &Pool<Postgres>,
    gcs_client: &google_cloud_storage::client::Storage,
    class_hash: &str,
) -> Result<(String, Option<ContractClass>)> {
    let verified_class = fetch_verified_class_with_inlining_class(db_pool, class_hash).await?;

    let primary_class_hash = &verified_class.0;
    let bucket = classes_bucket();

    // First try to fetch class data with inline_class_hash, if we have it
    if let Some(inline_class_hash) = &verified_class.1 {
        let key = key_for_class_hash(inline_class_hash);
        if let Ok(verified_class_data) = fetch_and_parse_file(gcs_client, &bucket, key).await {
            return Ok((
                primary_class_hash.clone(),
                Some(verified_class_data.contract_class),
            ));
        }
    }

    // If inline_class_hash does not exist, or there is no class data on GCS, try with primary_class_hash
    let key = key_for_class_hash(primary_class_hash);
    let contract_class_data = fetch_and_parse_file(gcs_client, &bucket, key)
        .await
        .ok()
        .map(|d| d.contract_class);

    Ok((primary_class_hash.clone(), contract_class_data))
}

pub async fn fetch_verified_class_hash_with_source_code_data(
    db_pool: &Pool<Postgres>,
    gcs_client: &google_cloud_storage::client::Storage,
    class_hash: &str,
) -> Result<Option<HashMap<String, String>>> {
    let verified_class = fetch_verified_class_with_inlining_class(db_pool, class_hash).await?;

    let primary_class_hash = &verified_class.0;
    let bucket = classes_bucket();

    // First try to fetch class data with inline_class_hash, if we have it
    if let Some(inline_class_hash) = &verified_class.1 {
        let key = key_for_class_hash(inline_class_hash);
        if let Ok(verified_class_data) = fetch_and_parse_file(gcs_client, &bucket, key).await {
            return Ok(Some(verified_class_data.source_code));
        }
    }

    // If inline_class_hash does not exist, or there is no class data on GCS, try with primary_class_hash
    let key = key_for_class_hash(primary_class_hash);
    let source_code_data = fetch_and_parse_file(gcs_client, &bucket, key)
        .await
        .ok()
        .map(|d| d.source_code);

    Ok(source_code_data)
}

pub async fn fetch_verified_class_with_data(
    db_pool: &Pool<Postgres>,
    gcs_client: &google_cloud_storage::client::Storage,
    class_hash: &String,
) -> Result<(VerifiedClassRow, VerifiedClassData)> {
    let verified_class = fetch_verified_class(db_pool, class_hash).await?;

    let parsed = read_object(
        gcs_client,
        &classes_bucket(),
        key_for_class_hash(class_hash),
    )
    .await?;
    let mut parsed: VerifiedClassData = serde_json::from_slice(&parsed)?;

    // Filter out unused files
    if let Some(cairo_debug_info) = &parsed.cairo_debug_info {
        let mut used_files: HashSet<String> = cairo_debug_info
            .sierra_statements_to_cairo_info
            .values()
            .flat_map(|info| info.cairo_locations.iter().map(|loc| loc.file_path.clone()))
            .collect();

        used_files.insert("Scarb.toml".to_string());

        parsed
            .source_code
            .retain(|file_path, _| used_files.contains(file_path));
    }

    Ok((verified_class, parsed))
}

async fn read_object(
    client: &google_cloud_storage::client::Storage,
    bucket: &str,
    key: String,
) -> Result<Vec<u8>> {
    let mut resp = client.read_object(bucket, key).send().await?;
    let mut body = Vec::new();
    while let Some(chunk) = resp.next().await.transpose()? {
        body.extend_from_slice(&chunk);
    }
    Ok(body)
}

async fn fetch_and_parse_file(
    client: &google_cloud_storage::client::Storage,
    bucket: &str,
    key: String,
) -> Result<VerifiedClassData> {
    let body = read_object(client, bucket, key).await?;
    let parsed: VerifiedClassData = serde_json::from_slice(&body)?;
    Ok(parsed)
}

/// Fetches the source code for a single class.
pub async fn fetch_class_source_code(
    gcs_client: &google_cloud_storage::client::Storage,
    class_hash: &String,
) -> Result<HashMap<String, String>> {
    let verified_class_data = match fetch_and_parse_file(
        gcs_client,
        &classes_bucket(),
        key_for_class_hash(class_hash),
    )
    .await
    {
        Ok(data) => data,
        Err(e) => {
            let error_message = format!(
                "Failed to fetch and parse file for class {}: {:?}",
                class_hash, e
            );
            error!(error_message);
            return Err(anyhow::anyhow!(error_message));
        }
    };

    Ok(verified_class_data.source_code)
}

pub async fn upload_class_to_gcs(
    gcs_client: &google_cloud_storage::client::Storage,
    class_hash: &str,
    contract_class: &ContractClass,
    cairo_debug_info: &Option<SierraToCairoDebugInfo>,
    source_code: &HashMap<String, String>,
) -> Result<()> {
    let verified_class_data = VerifiedClassData {
        contract_class: contract_class.clone(),
        cairo_debug_info: cairo_debug_info.clone(),
        source_code: source_code.clone(),
    };

    let json_data = serde_json::to_string(&verified_class_data)?;
    gcs_client
        .write_object(classes_bucket(), key_for_class_hash(class_hash), json_data)
        .send_unbuffered()
        .await?;

    Ok(())
}
