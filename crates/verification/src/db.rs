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

pub async fn fetch_verified_classes_with_inlining_classes(
    db_pool: &Pool<Postgres>,
    class_hashes: &[String],
) -> Result<HashMap<String, Option<String>>> {
    // Step 1: Fetch all contract classes that match the input class hashes
    let verified_classes = sqlx::query_as!(
        VerifiedClassRow,
        r#"
        SELECT *
        FROM contract_classes
        WHERE hash = ANY($1)
        "#,
        &class_hashes
    )
    .fetch_all(db_pool)
    .await?;

    // Collect all verified class hashes from contract_classes table
    let verified_class_hashes: Vec<String> = verified_classes
        .iter()
        .map(|c| c.hash.to_string())
        .collect();

    // Fetch verification_ids related to the verified class hashes
    let verification_ids_rows = sqlx::query!(
        r#"
        SELECT verification_id
        FROM class_hash_profiles
        WHERE class_hash = ANY($1)
        "#,
        &verified_class_hashes
    )
    .fetch_all(db_pool)
    .await?;

    // Initialize a map of class_hash -> None (assume no inlining by default)
    let mut class_hash_map: HashMap<String, Option<String>> = verified_class_hashes
        .iter()
        .map(|hash| (hash.clone(), None))
        .collect();

    // Collect all relevant verification_ids
    let verification_ids: Vec<Uuid> = verification_ids_rows
        .into_iter()
        .map(|row| row.verification_id)
        .collect();

    if !verification_ids.is_empty() {
        // Step 2: Fetch all inline_strategy_class_hash entries for verified classes
        let inline_class_hashes = sqlx::query!(
            r#"
            SELECT class_hash, inline_strategy_class_hash, verification_id
            FROM class_hash_profiles
            WHERE verification_id = ANY($1) AND class_hash = ANY($2)
            "#,
            &verification_ids,
            &verified_class_hashes
        )
        .fetch_all(db_pool)
        .await?;

        // Extract all inline_strategy_class_hash values that are not null
        let inline_hashes_to_check: Vec<String> = inline_class_hashes
            .iter()
            .filter_map(|row| row.inline_strategy_class_hash.clone())
            .collect();

        // Step 3: Check which inline_strategy_class_hash values are actually verified
        let existing_inline_hashes = sqlx::query!(
            r#"
            SELECT hash
            FROM contract_classes
            WHERE hash = ANY($1)
            "#,
            &inline_hashes_to_check
        )
        .fetch_all(db_pool)
        .await?;

        // Build a set of verified inline hashes
        let existing_inline_hash_set: std::collections::HashSet<String> = existing_inline_hashes
            .into_iter()
            .map(|row| row.hash)
            .collect();

        // Step 4: Update the map only if inline hash is also verified
        for row in inline_class_hashes {
            match row.inline_strategy_class_hash {
                Some(ref inline_hash) if existing_inline_hash_set.contains(inline_hash) => {
                    // Class is verified and its inlining pair is also verified — update map
                    class_hash_map.insert(row.class_hash.clone(), Some(inline_hash.clone()));
                }
                Some(_) => {
                    // Class has inlining pair, but that inlining pair is NOT verified — do not push
                    class_hash_map.remove(&row.class_hash);
                }
                None => {
                    // Class has no inlining pair (old Cairo version) — already defaulted to None
                }
            }
        }
    }

    Ok(class_hash_map)
}

pub async fn fetch_verified_class_with_inlining_class(
    db_pool: &Pool<Postgres>,
    class_hash: &str,
) -> Result<(String, Option<String>)> {
    // Step 1: Check if the provided class_hash exists in contract_classes (i.e. is verified)
    let verified_class = sqlx::query_as!(
        VerifiedClassRow,
        r#"
        SELECT *
        FROM contract_classes
        WHERE hash = $1
        "#,
        &class_hash
    )
    .fetch_optional(db_pool)
    .await?;

    if verified_class.is_none() {
        // If the class is not verified at all, return early with None
        return Ok((class_hash.to_string(), None));
    }

    let verified_class_hash = class_hash.to_string();

    // Step 2: Fetch verification_id(s) for this class from class_hash_profiles
    let verification_ids_rows = sqlx::query!(
        r#"
        SELECT verification_id
        FROM class_hash_profiles
        WHERE class_hash = $1
        "#,
        &verified_class_hash
    )
    .fetch_all(db_pool)
    .await?;

    let verification_ids: Vec<Uuid> = verification_ids_rows
        .into_iter()
        .map(|row| row.verification_id)
        .collect();

    if verification_ids.is_empty() {
        // Class has no verification_id (shouldn't happen if in contract_classes, but safe fallback)
        return Ok((verified_class_hash, None));
    }

    // Step 3: Get the inline strategy class hash, if it exists
    let inline_class_entry = sqlx::query!(
        r#"
        SELECT inline_strategy_class_hash
        FROM class_hash_profiles
        WHERE verification_id = ANY($1) AND class_hash = $2
        "#,
        &verification_ids,
        &verified_class_hash
    )
    .fetch_optional(db_pool)
    .await?;

    // Extract the inline_strategy_class_hash
    if let Some(row) = inline_class_entry {
        if let Some(ref inline_hash) = row.inline_strategy_class_hash {
            // Step 4: Check if inline_strategy_class_hash is verified
            let verified_inline = sqlx::query!(
                r#"
                SELECT hash
                FROM contract_classes
                WHERE hash = $1
                "#,
                inline_hash
            )
            .fetch_optional(db_pool)
            .await?;

            if verified_inline.is_some() {
                // Inline class is also verified
                return Ok((verified_class_hash, Some(inline_hash.clone())));
            }
        }
    }

    // No valid inline class or it is not verified
    Ok((verified_class_hash, None))
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
    is_inline_strategy_active: &bool,
    inline_strategy_class_hash: Option<&str>,
) -> Result<()> {
    sqlx::query!(
        r#"
        INSERT INTO class_hash_profiles (class_hash, profile, verification_id, is_inline_strategy_active, inline_strategy_class_hash)
        VALUES ($1, $2, $3, $4, $5)
        ON CONFLICT (class_hash, profile, verification_id) DO NOTHING
        "#,
        class_hash,
        profile,
        verification_id,
        is_inline_strategy_active,
        inline_strategy_class_hash
    )
    .execute(db_pool)
    .await?;

    Ok(())
}

/// Returns true if any verification_request linked to this class_hash via
/// class_hash_profiles has status='voyager' — i.e. the class was originally
/// fetched from Voyager and persisted by us, not verified through the regular
/// Walnut flow.
async fn class_has_voyager_provenance(db_pool: &Pool<Postgres>, class_hash: &str) -> Result<bool> {
    let row = sqlx::query!(
        r#"
        SELECT 1 AS hit
        FROM verification_requests vr
        JOIN class_hash_profiles chp ON chp.verification_id = vr.id
        WHERE chp.class_hash = $1 AND vr.status = 'voyager'
        LIMIT 1
        "#,
        class_hash
    )
    .fetch_optional(db_pool)
    .await?;

    Ok(row.is_some())
}

/// Build the provenance list for a class. "walnut" when it's verified locally;
/// "voyager" when we just fetched it from Voyager, or the local copy was
/// originally pulled from Voyager (verification_requests.status = 'voyager').
pub async fn build_class_sources(
    db_pool: &Pool<Postgres>,
    class_hash: &str,
    is_verified_locally: bool,
    voyager_hit: bool,
) -> Vec<String> {
    let mut sources = Vec::new();
    if is_verified_locally {
        sources.push("walnut".to_string());
        if !voyager_hit
            && class_has_voyager_provenance(db_pool, class_hash)
                .await
                .unwrap_or(false)
        {
            sources.push("voyager".to_string());
        }
    }
    if voyager_hit {
        sources.push("voyager".to_string());
    }
    sources
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

pub async fn fetch_inline_class_hashes_for_class_hashes(
    db_pool: &Pool<Postgres>,
    class_hashes: &[String],
) -> Result<HashMap<String, Option<String>>> {
    let rows = sqlx::query!(
        r#"
        SELECT class_hash, inline_strategy_class_hash
        FROM class_hash_profiles
        WHERE class_hash = ANY($1)
        "#,
        class_hashes,
    )
    .fetch_all(db_pool)
    .await?;

    let map = rows
        .into_iter()
        .map(|row| (row.class_hash, row.inline_strategy_class_hash))
        .collect();

    Ok(map)
}

pub async fn fetch_inline_class_hash_profiles_by_class_hash(
    db_pool: &Pool<Postgres>,
    class_hash: &str,
) -> Result<Option<String>, sqlx::Error> {
    let inline_class_hash = sqlx::query!(
        r#"
        SELECT inline_strategy_class_hash
        FROM class_hash_profiles
        WHERE class_hash = $1
        "#,
        class_hash
    )
    .fetch_optional(db_pool)
    .await?;

    Ok(inline_class_hash.and_then(|r| r.inline_strategy_class_hash))
}
