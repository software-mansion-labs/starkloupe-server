use crate::EVerificationStatus;
use crate::VerificationStatusRow;
use crate::VerifiedClassRow;
use anyhow::Result;
use sqlx::{Pool, Postgres};
use uuid::Uuid;

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

pub async fn is_class_verified(db_pool: &Pool<Postgres>, class_hash: String) -> Result<bool> {
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

pub async fn fetch_verification_id_and_status(
    db_pool: &Pool<Postgres>,
    class_hash: String,
    network: String,
) -> Result<Option<(Uuid, EVerificationStatus)>> {
    let result = sqlx::query!(
        r#"
        SELECT id, status as "status: EVerificationStatus"
        FROM verification_status
        WHERE class_hash = $1 AND network = $2 AND status IN ('pending', 'success')
        ORDER BY updated_at DESC
        LIMIT 1
        "#,
        class_hash,
        network
    )
    .fetch_optional(db_pool)
    .await?;

    // Return the result as an Option of a tuple
    Ok(result.map(|row| (row.id, row.status)))
}

pub async fn fetch_verification_status_data(
    db_pool: &Pool<Postgres>,
    id: Uuid,
) -> Result<VerificationStatusRow, sqlx::Error> {
    let verification_status = sqlx::query_as!(
        VerificationStatusRow,
        r#"SELECT id, network, class_hash, status as "status: EVerificationStatus", error_message, created_at, updated_at
        FROM verification_status
        WHERE id = $1"#,
        &id
    )
    .fetch_one(db_pool)
    .await?;

    Ok(verification_status)
}
