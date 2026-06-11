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

pub async fn list_members(
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
