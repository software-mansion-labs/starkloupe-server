use crate::{ClassDebuggerData, ClassDebuggerDataWithContractClass};
use anyhow::Result;
use cairo_annotations::annotations::coverage::VersionedCoverageAnnotations;
use cairo_annotations::annotations::TryFromDebugInfo;
use futures::future;
use itertools::Itertools;
use sqlx::{Pool, Postgres};
use std::collections::HashMap;
use tracing::error;
use verification::{
    db::fetch_verified_classes_with_inlining_classes, s3::key_for_class_hash, CodeLocation,
    SierraStatementToCairoDebugInfo, VerifiedClassData,
};

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
    classes: &[String],
) -> HashMap<String, ClassDebuggerDataWithContractClass> {
    let mut classes_debugger_data: HashMap<String, ClassDebuggerDataWithContractClass> =
        HashMap::new();

    let verified_classes =
        match fetch_verified_classes_with_inlining_classes(db_pool, classes).await {
            Ok(vc) => vc,
            Err(e) => {
                error!("Failed to fetch verified classes: {:?}", e);
                HashMap::new()
            }
        };

    let fetches = verified_classes.iter().map(|(key, value)| match value {
        Some(value) => fetch_and_parse_file(
            s3_client,
            "walnutserver-east-1-classes-verification",
            key_for_class_hash(value),
        ),
        None => fetch_and_parse_file(
            s3_client,
            "walnutserver-east-1-classes-verification",
            key_for_class_hash(key),
        ),
    });

    let results = match future::try_join_all(fetches).await {
        Ok(results) => results,
        Err(e) => {
            error!("Failed to fetch and parse files: {:?}", e);
            Vec::new()
        }
    };

    for (verified_class_row, verified_class_data) in verified_classes.iter().zip(results.iter()) {
        let class_debugger_data = if let Some(cairo_debug_info) =
            &verified_class_data.cairo_debug_info
        {
            Some(ClassDebuggerData {
                sierra_statements_to_cairo_info: cairo_debug_info
                    .sierra_statements_to_cairo_info
                    .clone(),
                source_code: verified_class_data.source_code.clone(),
            })
        } else {
            if let Some(debug_info) = &verified_class_data.contract_class.sierra_program_debug_info
            {
                match VersionedCoverageAnnotations::try_from_debug_info(debug_info) {
                    Ok(annotations) => match annotations {
                        VersionedCoverageAnnotations::V1(annotations) => {
                            let mut sierra_statements_to_cairo_info: HashMap<
                                usize,
                                SierraStatementToCairoDebugInfo,
                            > = HashMap::new();
                            for (id, code_locations) in annotations.statements_code_locations {
                                sierra_statements_to_cairo_info.insert(
                                    id.0,
                                    SierraStatementToCairoDebugInfo {
                                        cairo_locations: code_locations
                                            .into_iter()
                                            .filter_map(|c| {
                                                let mut code_location =
                                                    CodeLocation::from_coverage(c);
                                                if code_location
                                                    .file_path
                                                    .contains("tmp/verification")
                                                {
                                                    if code_location
                                                        .file_path
                                                        .ends_with("[contract]")
                                                    {
                                                        code_location.file_path = code_location
                                                            .file_path
                                                            .replace("[contract]", "");
                                                    }
                                                    if let Some(pos) =
                                                        code_location.file_path.find("/src/")
                                                    {
                                                        code_location.file_path = code_location
                                                            .file_path[(pos + 1)..]
                                                            .to_string();
                                                    }
                                                    Some(code_location)
                                                } else {
                                                    None
                                                }
                                            })
                                            .collect_vec(),
                                    },
                                );
                            }
                            Some(ClassDebuggerData {
                                sierra_statements_to_cairo_info,
                                source_code: verified_class_data.source_code.clone(),
                            })
                        }
                    },
                    Err(e) => {
                        error!("Failed to parse coverage info: {:?}", e);
                        None
                    }
                }
            } else {
                None
            }
        };

        classes_debugger_data.insert(
            verified_class_row.0.clone(),
            ClassDebuggerDataWithContractClass {
                inline_strategy_class_hash: verified_class_row.1.clone(),
                class_debugger_data,
                contract_class: verified_class_data.contract_class.clone(),
            },
        );
    }

    classes_debugger_data
}
