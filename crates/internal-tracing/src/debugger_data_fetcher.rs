use crate::{ClassDebuggerData, ClassDebuggerDataWithContractClass};
use anyhow::Result;
use futures::future;
use sqlx::{Pool, Postgres};
use std::collections::HashMap;
use tracing::error;
use verification::{db::fetch_verified_classes, s3::key_for_class_hash, VerifiedClassData};

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

/// Fetches the debugger data for the given classes.
pub async fn fetch_classes_debugger_data(
    db_pool: &Pool<Postgres>,
    s3_client: &aws_sdk_s3::Client,
    classes: Vec<String>,
) -> HashMap<String, ClassDebuggerDataWithContractClass> {
    let mut classes_debugger_data: HashMap<String, ClassDebuggerDataWithContractClass> =
        HashMap::new();

    let verified_classes = fetch_verified_classes(db_pool, classes.clone())
        .await
        .unwrap();

    let fetches = verified_classes.iter().map(|verified_class| {
        fetch_and_parse_file(
            s3_client,
            "walnutserver-east-1-classes-verification",
            key_for_class_hash(&verified_class.hash),
        )
    });

    let results = match future::try_join_all(fetches).await {
        Ok(results) => results,
        Err(e) => {
            error!("Failed to fetch and parse files: {:?}", e);
            Vec::new()
        }
    };

    for (verified_class_row, verified_class_data) in verified_classes.iter().zip(results.iter()) {
        let class_debugger_data =
            if let Some(cairo_debug_info) = &verified_class_data.cairo_debug_info {
                Some(ClassDebuggerData {
                    sierra_statements_to_cairo_info: cairo_debug_info
                        .sierra_statements_to_cairo_info
                        .clone(),
                    source_code: verified_class_data.source_code.clone(),
                })
            } else {
                None
            };

        classes_debugger_data.insert(
            verified_class_row.hash.clone(),
            ClassDebuggerDataWithContractClass {
                class_debugger_data,
                contract_class: verified_class_data.contract_class.clone(),
            },
        );
    }

    classes_debugger_data
}
