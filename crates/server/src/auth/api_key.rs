use axum::{
    async_trait,
    extract::FromRequestParts,
    http::{request::Parts, HeaderMap, StatusCode},
};
use sha2::{Digest, Sha256};
use std::sync::Arc;
use uuid::Uuid;

use crate::app_state::AppState;

const HEADER_NAME: &str = "x-api-key";
const LIVE_PREFIX: &str = "wk_live_";
const DEVNET_PREFIX: &str = "dt_";

#[derive(Debug, Clone)]
pub struct ApiKeyAuth {
    // Read by M1 handlers to scope queries per tenant; not consumed yet.
    #[allow(dead_code)]
    pub tenant_id: Uuid,
}

fn validate_shape(value: &str) -> Result<(), StatusCode> {
    if value.starts_with(DEVNET_PREFIX) {
        return Err(StatusCode::UNAUTHORIZED);
    }
    if !value.starts_with(LIVE_PREFIX) {
        return Err(StatusCode::UNAUTHORIZED);
    }
    Ok(())
}

fn hash_key(value: &str) -> [u8; 32] {
    Sha256::digest(value.as_bytes()).into()
}

fn decide(row: Option<(Uuid, String)>) -> Result<Uuid, StatusCode> {
    match row {
        None => Err(StatusCode::UNAUTHORIZED),
        Some((_, status)) if status != "active" => Err(StatusCode::UNAUTHORIZED),
        Some((tenant_id, _)) => Ok(tenant_id),
    }
}

fn extract_key(headers: &HeaderMap) -> Result<String, StatusCode> {
    let header = headers.get(HEADER_NAME).ok_or(StatusCode::UNAUTHORIZED)?;
    let value = header.to_str().map_err(|_| StatusCode::UNAUTHORIZED)?;
    validate_shape(value)?;
    Ok(value.to_string())
}

async fn authenticate(headers: &HeaderMap, state: &AppState) -> Result<Uuid, StatusCode> {
    let key = extract_key(headers)?;
    let hash = hash_key(&key);

    if let Some(tenant_id) = state.api_key_cache.get(&hash).await {
        return Ok(tenant_id);
    }

    let row = sqlx::query!(
        "SELECT tenant_id, status FROM api_keys WHERE key_hash = $1",
        &hash as &[u8],
    )
    .fetch_optional(&state.db_pool)
    .await
    .map_err(|e| {
        tracing::error!("api_key lookup failed: {}", e);
        StatusCode::UNAUTHORIZED
    })?;

    let tenant_id = decide(row.map(|r| (r.tenant_id, r.status)))?;

    state.api_key_cache.insert(hash, tenant_id).await;

    let pool = state.db_pool.clone();
    tokio::spawn(async move {
        if let Err(e) = sqlx::query!(
            "UPDATE api_keys SET last_used_at = now() WHERE key_hash = $1",
            &hash as &[u8],
        )
        .execute(&pool)
        .await
        {
            tracing::warn!("best-effort last_used_at update failed: {}", e);
        }
    });

    Ok(tenant_id)
}

#[async_trait]
impl FromRequestParts<AppState> for ApiKeyAuth {
    type Rejection = StatusCode;

    async fn from_request_parts(
        parts: &mut Parts,
        state: &AppState,
    ) -> Result<Self, Self::Rejection> {
        let tenant_id = authenticate(&parts.headers, state).await?;
        Ok(ApiKeyAuth { tenant_id })
    }
}

#[async_trait]
impl FromRequestParts<Arc<AppState>> for ApiKeyAuth {
    type Rejection = StatusCode;

    async fn from_request_parts(
        parts: &mut Parts,
        state: &Arc<AppState>,
    ) -> Result<Self, Self::Rejection> {
        let tenant_id = authenticate(&parts.headers, state).await?;
        Ok(ApiKeyAuth { tenant_id })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::auth::gen_token;

    #[test]
    fn rejects_devnet_token_prefix() {
        // Token-confusion guard: dt_ belongs to M1.
        assert_eq!(
            validate_shape("dt_abc123").unwrap_err(),
            StatusCode::UNAUTHORIZED
        );
    }

    #[test]
    fn rejects_wrong_shape() {
        assert_eq!(
            validate_shape("garbage").unwrap_err(),
            StatusCode::UNAUTHORIZED
        );
        assert_eq!(validate_shape("").unwrap_err(), StatusCode::UNAUTHORIZED);
    }

    #[test]
    fn accepts_live_shape() {
        assert!(validate_shape("wk_live_anything").is_ok());
    }

    #[test]
    fn generated_live_token_passes_shape() {
        let token = gen_token(LIVE_PREFIX);
        assert!(validate_shape(&token.plaintext).is_ok());
    }

    #[test]
    fn hash_matches_gen_token() {
        let token = gen_token(LIVE_PREFIX);
        assert_eq!(hash_key(&token.plaintext), token.hash);
    }

    #[test]
    fn missing_header_is_unauthorized() {
        let headers = HeaderMap::new();
        assert_eq!(extract_key(&headers).unwrap_err(), StatusCode::UNAUTHORIZED);
    }

    #[test]
    fn decide_no_match_is_unauthorized() {
        assert_eq!(decide(None).unwrap_err(), StatusCode::UNAUTHORIZED);
    }

    #[test]
    fn decide_revoked_is_unauthorized() {
        let row = Some((Uuid::new_v4(), "revoked".to_string()));
        assert_eq!(decide(row).unwrap_err(), StatusCode::UNAUTHORIZED);
    }

    #[test]
    fn decide_active_returns_tenant() {
        let id = Uuid::new_v4();
        let row = Some((id, "active".to_string()));
        assert_eq!(decide(row).unwrap(), id);
    }
}
