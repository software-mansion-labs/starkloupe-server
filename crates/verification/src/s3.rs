use anyhow::Result;
use sqlx::{Pool, Postgres};
use std::collections::HashSet;

use crate::db::fetch_verified_class;
use crate::{VerifiedClassData, VerifiedClassRow};

pub fn key_for_class_hash(class_hash: String) -> String {
    format!("class-{}.json", class_hash)
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
