use anyhow::{Context, Result};
use aws_sdk_s3::primitives::ByteStream;
use aws_smithy_types::body::SdkBody;
use sqlx::{Pool, Postgres};
use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;
use tracing::error;
use uuid::Uuid;
use walnut_shared::tuple_to_version_string;

use crate::manifest::Manifest;
use crate::scarb::build_with_scarb;
use crate::utils::create_files_from_map;
use crate::verification::move_failed_verification_to_failed_tmp;
use crate::VerifiedClassData;

pub async fn initiate_minimal_verification(
    db_pool: &Pool<Postgres>,
    s3_client: &aws_sdk_s3::Client,
    source_code: HashMap<String, String>,
    manifest: Manifest,
) -> Result<Uuid> {
    let verification_request_id = Uuid::new_v4();

    sqlx::query!(
        r#"
        INSERT INTO verification_requests (id, status, cairo_version, package_name)
        VALUES ($1, $2, $3, $4)
        "#,
        verification_request_id,
        "pending",
        tuple_to_version_string(manifest.cairo_version),
        manifest.package_name
    )
    .execute(db_pool)
    .await
    .context("Failed to insert verification status entry")?;

    let db_pool_clone = db_pool.clone();
    let s3_client_clone = s3_client.clone();

    tokio::spawn(async move {
        match verify(&db_pool_clone, &s3_client_clone, source_code, manifest).await {
            Ok((verified_now_hashes, verified_before_hashes)) => {
                if let Err(e) = sqlx::query!(
                    r#"
                UPDATE verification_requests
                SET status = 'success', updated_at = NOW()
                WHERE id = $1
                "#,
                    verification_request_id
                )
                .execute(&db_pool_clone)
                .await
                {
                    error!("Failed to update verification request status: {:?}", e);
                }

                for class_hash in &verified_now_hashes {
                    if let Err(e) = sqlx::query!(
                        r#"
                        INSERT INTO verification_status (id, class_hash, status)
                        VALUES ($1, $2, $3)
                        "#,
                        verification_request_id,
                        class_hash,
                        "success"
                    )
                    .execute(&db_pool_clone)
                    .await
                    .context("Failed to insert verification status entry")
                    {
                        error!("Failed to insert verification status entry: {:?}", e);
                    }
                }

                for class_hash in &verified_before_hashes {
                    if let Err(e) = sqlx::query!(
                        r#"
                        INSERT INTO verification_status (id, class_hash, status, message)
                        VALUES ($1, $2, $3, $4)
                        "#,
                        verification_request_id,
                        class_hash,
                        "success",
                        "This class is already verified."
                    )
                    .execute(&db_pool_clone)
                    .await
                    {
                        error!("Failed to insert verification status entry: {:?}", e);
                    }
                }
            }
            Err(e) => {
                if let Err(err) = sqlx::query!(
                    r#"
                UPDATE verification_requests
                SET status = 'failed', updated_at = NOW(), message = $1
                WHERE id = $2
                "#,
                    e.to_string(),
                    verification_request_id
                )
                .execute(&db_pool_clone)
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
    source_code: HashMap<String, String>,
    manifest: Manifest,
) -> Result<(Vec<String>, Vec<String>)> {
    let random_string = Uuid::new_v4().to_string();
    let mut tmp_dir = PathBuf::from("tmp/verification");
    tmp_dir.push(&random_string);

    create_files_from_map(&source_code, &tmp_dir)?;

    let classes = build_with_scarb(manifest, &tmp_dir);
    match &classes {
        Ok(classes) => {
            fs::remove_dir_all(&tmp_dir)?;
        },
        Err(e) => {
            move_failed_verification_to_failed_tmp(&tmp_dir, &random_string, e).await?;
        }
    }

    let classes = classes?;

    let class_hashes: Vec<String> = classes
        .iter()
        .map(|(class_hash, _)| class_hash.clone())
        .collect();

    let verified_contract_classes: Vec<String> = sqlx::query!(
        r#"
        SELECT hash
        FROM contract_classes
        WHERE hash = ANY($1)
        "#,
        &class_hashes
    )
    .fetch_all(db_pool)
    .await?
    .into_iter()
    .map(|row| row.hash)
    .collect();

    let mut class_hashes_to_verify = Vec::new();

    for (class_hash, class) in classes {
        if verified_contract_classes.contains(&class_hash) {
            continue;
        } else {
            class_hashes_to_verify.push(class_hash.clone());
        }

        let verified_class_data = VerifiedClassData {
            contract_class: class,
            cairo_debug_info: None,
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

        let _rec = sqlx::query!(
                r#"
                INSERT INTO contract_classes ( hash, is_sierra_debug_info, is_cairo_debug_info, is_source_code)
                VALUES ( $1, $2, $3, $4 )
                        "#,
                        class_hash,
                    true,
                    true,
                    true,
                )
                .execute(db_pool)
                .await?;
    }

    Ok((class_hashes_to_verify, verified_contract_classes))
}
