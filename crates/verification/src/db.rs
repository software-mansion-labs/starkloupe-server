use anyhow::Result;
use sqlx::{Pool, Postgres};

use crate::VerifiedClassRow;

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
