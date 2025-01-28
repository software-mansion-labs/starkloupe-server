use crate::db::fetch_verified_class;
use crate::SierraToCairoDebugInfo;
use crate::{VerifiedClassData, VerifiedClassRow};
use anyhow::Result;
use aws_sdk_s3::primitives::ByteStream;
use aws_smithy_types::body::SdkBody;
use cairo_lang_starknet_classes::contract_class::ContractClass;
use sqlx::{Pool, Postgres};
use std::collections::{HashMap, HashSet};
use tracing::error;

pub fn key_for_class_hash(class_hash: &str) -> String {
    format!("class-{}.json", class_hash)
}

pub async fn fetch_verified_class_with_data(
    db_pool: &Pool<Postgres>,
    s3_client: &aws_sdk_s3::Client,
    class_hash: &String,
) -> Result<(VerifiedClassRow, VerifiedClassData)> {
    let verified_class = fetch_verified_class(db_pool, class_hash).await?;

    let resp = s3_client
        .get_object()
        .bucket("walnutserver-east-1-classes-verification")
        .key(key_for_class_hash(class_hash))
        .send()
        .await?;

    let body = resp.body.collect().await?;
    let mut parsed: VerifiedClassData = serde_json::from_slice(&body.into_bytes())?;

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

async fn fetch_and_parse_file(
    client: &aws_sdk_s3::Client,
    bucket_name: &str,
    key: String,
) -> Result<VerifiedClassData> {
    let resp = client
        .get_object()
        .bucket(bucket_name)
        .key(key)
        .send()
        .await?;

    let body = resp.body.collect().await?;
    let parsed: VerifiedClassData = serde_json::from_slice(&body.into_bytes())?;
    Ok(parsed)
}

/// Fetches the source code for a single class.
pub async fn fetch_class_source_code(
    s3_client: &aws_sdk_s3::Client,
    class_hash: &String,
) -> Result<HashMap<String, String>> {
    let verified_class_data = match fetch_and_parse_file(
        s3_client,
        "walnutserver-east-1-classes-verification",
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

pub async fn upload_class_to_s3(
    s3_client: &aws_sdk_s3::Client,
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
    s3_client
        .put_object()
        .bucket("walnutserver-east-1-classes-verification")
        .key(format!("class-{}.json", class_hash))
        .body(ByteStream::new(SdkBody::from(json_data)))
        .send()
        .await?;

    Ok(())
}
