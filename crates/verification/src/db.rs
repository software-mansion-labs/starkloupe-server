use crate::EVerificationStatus;
use crate::VerificationRequestRow;
use crate::VerificationStatusRow;
use crate::VerifiedClassRow;
use anyhow::Result;
use sqlx::{Pool, Postgres};
use std::collections::HashMap;
use uuid::Uuid;

pub async fn fetch_verified_classes(
    db_pool: &Pool<Postgres>,
    class_hashes: &[String],
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
    class_hash: &String,
) -> Result<VerifiedClassRow> {
    let verified_class = sqlx::query_as!(
        VerifiedClassRow,
        r#"SELECT *
        FROM contract_classes
        WHERE hash = $1"#,
        class_hash
    )
    .fetch_one(db_pool)
    .await?;

    Ok(verified_class)
}

pub async fn insert_contract_class(
    db_pool: &Pool<Postgres>,
    class_hash: &str,
    is_sierra_debug_info: bool,
    is_cairo_debug_info: bool,
    is_source_code: bool,
    network: Option<&str>,
) -> Result<()> {
    sqlx::query!(
                r#"
                INSERT INTO contract_classes (hash, is_sierra_debug_info, is_cairo_debug_info, is_source_code, chain_id)
                VALUES ($1, $2, $3, $4, $5) ON CONFLICT (hash) DO NOTHING
                "#,
                class_hash,
                is_sierra_debug_info,
                is_cairo_debug_info,
                is_source_code,
                network
            )
            .execute(db_pool)
            .await?;

    Ok(())
}

pub async fn insert_class_hash_profiles(
    db_pool: &Pool<Postgres>,
    class_hash: &str,
    profile: &str,
    verification_id: Uuid,
) -> Result<()> {
    sqlx::query!(
        r#"
        INSERT INTO class_hash_profiles (class_hash, profile, verification_id)
        VALUES ($1, $2, $3)
        ON CONFLICT (class_hash, profile, verification_id) DO NOTHING
        "#,
        class_hash,
        profile,
        verification_id
    )
    .execute(db_pool)
    .await?;

    Ok(())
}

pub async fn insert_verification_request(
    db_pool: &Pool<Postgres>,
    verification_request_id: Uuid,
    status: &str,
    cairo_version: &str,
    package_name: &str,
) -> Result<()> {
    sqlx::query!(
        r#"
        INSERT INTO verification_requests (id, status, cairo_version, package_name)
        VALUES ($1, $2, $3, $4)
        "#,
        verification_request_id,
        status,
        cairo_version,
        package_name
    )
    .execute(db_pool)
    .await?;

    Ok(())
}

pub async fn update_verification_request(
    db_pool: &Pool<Postgres>,
    verification_request_id: Uuid,
    status: &str,
    error_message: &Option<String>,
) -> Result<()> {
    match error_message {
        Some(error) => {
            sqlx::query!(
                r#"
                UPDATE verification_requests
                SET status = $1, updated_at = NOW(), message = $2
                WHERE id = $3
                "#,
                status,
                error,
                verification_request_id
            )
            .execute(db_pool)
            .await?;
        }
        None => {
            sqlx::query!(
                r#"
                UPDATE verification_requests
                SET status = $1, updated_at = NOW()
                WHERE id = $2
                "#,
                status,
                verification_request_id
            )
            .execute(db_pool)
            .await?;
        }
    }

    Ok(())
}

pub async fn fetch_verification_statuses_pending_or_success(
    db_pool: &Pool<Postgres>,
    class_hashes: &[String],
) -> Result<Vec<(String, EVerificationStatus)>> {
    let rows = sqlx::query!(
        r#"
        SELECT class_hash, status as "status: EVerificationStatus"
        FROM verification_status
        WHERE class_hash = ANY($1) AND status IN ('pending', 'success')
        ORDER BY updated_at DESC
        "#,
        class_hashes
    )
    .fetch_all(db_pool)
    .await?;

    let results = rows
        .into_iter()
        .filter_map(|row| {
            let class_hash = row.class_hash?;
            let status = row.status;
            Some((class_hash, status))
        })
        .collect();

    Ok(results)
}

pub async fn fetch_verification_statuses_by_id(
    db_pool: &Pool<Postgres>,
    id: Uuid,
) -> Result<(Option<VerificationRequestRow>, Vec<VerificationStatusRow>), sqlx::Error> {
    let verification_request = sqlx::query_as!(
        VerificationRequestRow,
        r#"SELECT id, status, message, created_at, updated_at, cairo_version, package_name
        FROM verification_requests
        WHERE id = $1"#,
        &id
    )
    .fetch_optional(db_pool)
    .await?;

    let verification_statuses = sqlx::query_as!(
        VerificationStatusRow,
        r#"SELECT primary_id, id, network, class_hash, status as "status: EVerificationStatus", message, created_at, updated_at, project_id
        FROM verification_status
        WHERE id = $1
        ORDER BY primary_id"#,
        &id
    )
    .fetch_all(db_pool)
    .await?;

    Ok((verification_request, verification_statuses))
}

pub async fn insert_verification_status(
    db_pool: &Pool<Postgres>,
    verification_id: Uuid,
    class_hash: &str,
    status: &str,
    message: Option<&str>,
    network: Option<&str>,
) -> Result<()> {
    sqlx::query!(
        r#"
                INSERT INTO verification_status (id, class_hash, status, message, network)
                VALUES ($1, $2, $3, $4, $5)
                "#,
        verification_id,
        class_hash,
        status,
        message,
        network
    )
    .execute(db_pool)
    .await?;

    Ok(())
}

pub async fn update_verification_status(
    db_pool: &Pool<Postgres>,
    verification_id: Uuid,
    class_hash: &str,
    status: &str,
    message: &Option<String>,
) -> Result<()> {
    match message {
        Some(message) => {
            sqlx::query!(
                r#"
                UPDATE verification_status
                SET status = $1, message = $2
                WHERE id = $3 AND class_hash = $4
                "#,
                status,
                message,
                verification_id,
                class_hash
            )
            .execute(db_pool)
            .await?;
        }
        None => {
            sqlx::query!(
                r#"
                UPDATE verification_status
                SET status = $1
                WHERE id = $2 AND class_hash = $3
                "#,
                status,
                verification_id,
                class_hash
            )
            .execute(db_pool)
            .await?;
        }
    }

    Ok(())
}

pub async fn update_verification_statuses(
    db_pool: &Pool<Postgres>,
    verification_id: Uuid,
    class_hashes: &[String],
    status: &str,
    message: &Option<String>,
) -> Result<()> {
    match message {
        Some(message) => {
            sqlx::query!(
                r#"
                UPDATE verification_status
                SET status = $1, message = $2
                WHERE id = $3 AND class_hash = ANY($4)
                "#,
                status,
                message,
                verification_id,
                class_hashes
            )
            .execute(db_pool)
            .await?;
        }
        None => {
            sqlx::query!(
                r#"
                UPDATE verification_status
                SET status = $1
                WHERE id = $2 AND class_hash = ANY($3)
                "#,
                status,
                verification_id,
                class_hashes
            )
            .execute(db_pool)
            .await?;
        }
    }

    Ok(())
}

pub async fn fetch_class_hash_profiles_by_id(
    db_pool: &Pool<Postgres>,
    verification_id: Uuid,
) -> Result<HashMap<String, Vec<String>>, sqlx::Error> {
    let rows = sqlx::query!(
        r#"
        SELECT class_hash, ARRAY_AGG(profile) AS profiles
        FROM class_hash_profiles
        WHERE verification_id = $1
        GROUP BY class_hash
        "#,
        verification_id
    )
    .fetch_all(db_pool)
    .await?;

    let mut class_hash_map: HashMap<String, Vec<String>> = HashMap::new();

    for row in rows {
        class_hash_map.insert(row.class_hash, row.profiles.unwrap_or_default());
    }

    Ok(class_hash_map)
}
