use sqlx::{Pool, Postgres};
use time::OffsetDateTime;
use uuid::Uuid;

pub struct Tenant {
    pub id: Uuid,
    pub name: String,
    pub created_at: OffsetDateTime,
}

pub struct TenantMember {
    pub id: Uuid,
    pub tenant_id: Uuid,
    pub github_email: String,
    pub added_at: OffsetDateTime,
}

pub struct ApiKeyCreated {
    pub id: Uuid,
    pub tenant_id: Uuid,
    pub created_at: OffsetDateTime,
}

pub struct ApiKeyListRow {
    pub id: Uuid,
    pub key_prefix: String,
    pub status: String,
    pub created_at: OffsetDateTime,
    pub created_by_email: String,
    pub revoked_at: Option<OffsetDateTime>,
    pub revoked_by_email: Option<String>,
}

pub struct ActiveApiKey {
    pub id: Uuid,
    pub key_prefix: String,
    pub status: String,
    pub created_at: OffsetDateTime,
}

pub async fn create_tenant(pool: &Pool<Postgres>, name: &str) -> Result<Tenant, sqlx::Error> {
    sqlx::query_as!(
        Tenant,
        "INSERT INTO tenants (name) VALUES ($1) RETURNING id, name, created_at",
        name
    )
    .fetch_one(pool)
    .await
}

pub async fn add_member(
    pool: &Pool<Postgres>,
    tenant_id: Uuid,
    github_email: &str,
    added_by_email: &str,
) -> Result<TenantMember, sqlx::Error> {
    sqlx::query_as!(
        TenantMember,
        "INSERT INTO tenant_members (tenant_id, github_email, added_by_email) \
         VALUES ($1, $2, $3) \
         RETURNING id, tenant_id, github_email, added_at",
        tenant_id,
        github_email,
        added_by_email
    )
    .fetch_one(pool)
    .await
}

pub async fn remove_member(
    pool: &Pool<Postgres>,
    tenant_id: Uuid,
    member_id: Uuid,
    removed_by_email: &str,
) -> Result<u64, sqlx::Error> {
    let result = sqlx::query!(
        "UPDATE tenant_members \
         SET removed_at = now(), removed_by_email = $1 \
         WHERE id = $2 AND tenant_id = $3 AND removed_at IS NULL",
        removed_by_email,
        member_id,
        tenant_id
    )
    .execute(pool)
    .await?;

    Ok(result.rows_affected())
}

pub async fn list_tenant_members(
    pool: &Pool<Postgres>,
    tenant_id: Uuid,
) -> Result<Vec<TenantMember>, sqlx::Error> {
    sqlx::query_as!(
        TenantMember,
        "SELECT id, tenant_id, github_email, added_at FROM tenant_members \
         WHERE tenant_id = $1 AND removed_at IS NULL \
         ORDER BY added_at",
        tenant_id
    )
    .fetch_all(pool)
    .await
}

pub async fn get_tenant(pool: &Pool<Postgres>, id: Uuid) -> Result<Option<Tenant>, sqlx::Error> {
    sqlx::query_as!(
        Tenant,
        "SELECT id, name, created_at FROM tenants WHERE id = $1",
        id
    )
    .fetch_optional(pool)
    .await
}

pub async fn create_api_key(
    pool: &Pool<Postgres>,
    tenant_id: Uuid,
    key_hash: &[u8],
    key_prefix: &str,
    created_by_email: &str,
) -> Result<ApiKeyCreated, sqlx::Error> {
    sqlx::query_as!(
        ApiKeyCreated,
        "INSERT INTO api_keys (tenant_id, key_hash, key_prefix, created_by_email) \
         VALUES ($1, $2, $3, $4) \
         RETURNING id, tenant_id, created_at",
        tenant_id,
        key_hash,
        key_prefix,
        created_by_email
    )
    .fetch_one(pool)
    .await
}

pub async fn revoke_api_key(
    pool: &Pool<Postgres>,
    id: Uuid,
    revoked_by_email: &str,
) -> Result<Option<Vec<u8>>, sqlx::Error> {
    let row = sqlx::query!(
        "UPDATE api_keys \
         SET status = 'revoked', revoked_at = now(), revoked_by_email = $1 \
         WHERE id = $2 AND status = 'active' \
         RETURNING key_hash",
        revoked_by_email,
        id
    )
    .fetch_optional(pool)
    .await?;

    Ok(row.map(|r| r.key_hash))
}

pub async fn list_api_keys(
    pool: &Pool<Postgres>,
    tenant_id: Uuid,
) -> Result<Vec<ApiKeyListRow>, sqlx::Error> {
    sqlx::query_as!(
        ApiKeyListRow,
        "SELECT id, key_prefix, status, created_at, created_by_email, revoked_at, revoked_by_email \
         FROM api_keys WHERE tenant_id = $1 \
         ORDER BY created_at",
        tenant_id
    )
    .fetch_all(pool)
    .await
}

pub async fn get_active_api_key(
    pool: &Pool<Postgres>,
    tenant_id: Uuid,
) -> Result<Option<ActiveApiKey>, sqlx::Error> {
    sqlx::query_as!(
        ActiveApiKey,
        "SELECT id, key_prefix, status, created_at FROM api_keys \
         WHERE tenant_id = $1 AND status = 'active'",
        tenant_id
    )
    .fetch_optional(pool)
    .await
}
