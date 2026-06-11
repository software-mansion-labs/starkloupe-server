use crate::app_state::AppState;
use crate::auth::admin_token::AdminAuth;
use crate::auth::gen_token;
use axum::{
    extract::{Path, Query, State},
    http::StatusCode,
    response::{IntoResponse, Response},
    Json,
};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use time::OffsetDateTime;
use uuid::Uuid;

pub enum AdminError {
    Validation(&'static str),
    Conflict,
    NotFound,
    Internal,
}

impl From<sqlx::Error> for AdminError {
    fn from(e: sqlx::Error) -> Self {
        match &e {
            sqlx::Error::Database(db) if db.is_unique_violation() => AdminError::Conflict,
            sqlx::Error::Database(db) if db.is_foreign_key_violation() => AdminError::NotFound,
            sqlx::Error::Database(db) if db.is_check_violation() => {
                AdminError::Validation("constraint violation")
            }
            _ => {
                tracing::error!("admin db error: {e}");
                AdminError::Internal
            }
        }
    }
}

impl IntoResponse for AdminError {
    fn into_response(self) -> Response {
        match self {
            AdminError::Validation(message) => (StatusCode::BAD_REQUEST, message).into_response(),
            AdminError::Conflict => StatusCode::CONFLICT.into_response(),
            AdminError::NotFound => StatusCode::NOT_FOUND.into_response(),
            AdminError::Internal => StatusCode::INTERNAL_SERVER_ERROR.into_response(),
        }
    }
}

fn validate_required(value: &str, message: &'static str) -> Result<(), AdminError> {
    if value.trim().is_empty() {
        Err(AdminError::Validation(message))
    } else {
        Ok(())
    }
}

#[derive(Deserialize)]
pub struct CreateTenantBody {
    pub name: String,
}

#[derive(Serialize)]
pub struct TenantResponse {
    pub id: Uuid,
    pub name: String,
    #[serde(with = "time::serde::rfc3339")]
    pub created_at: OffsetDateTime,
}

#[derive(Deserialize)]
pub struct AddMemberBody {
    pub github_email: String,
    pub added_by_email: String,
}

#[derive(Serialize)]
pub struct MemberResponse {
    pub id: Uuid,
    pub tenant_id: Uuid,
    pub github_email: String,
    #[serde(with = "time::serde::rfc3339")]
    pub added_at: OffsetDateTime,
}

#[derive(Deserialize)]
pub struct RemoveMemberBody {
    pub removed_by_email: String,
}

#[derive(Deserialize)]
pub struct CreateApiKeyBody {
    pub tenant_id: Uuid,
    pub created_by_email: String,
}

#[derive(Serialize)]
pub struct CreateApiKeyResponse {
    pub id: Uuid,
    pub tenant_id: Uuid,
    pub plaintext: String,
    pub prefix: String,
    #[serde(with = "time::serde::rfc3339")]
    pub created_at: OffsetDateTime,
}

#[derive(Deserialize)]
pub struct RevokeApiKeyBody {
    pub revoked_by_email: String,
}

#[derive(Deserialize)]
pub struct ListApiKeysParams {
    pub tenant_id: Uuid,
}

#[derive(Serialize)]
pub struct ApiKeyListItem {
    pub id: Uuid,
    pub prefix: String,
    pub status: String,
    #[serde(with = "time::serde::rfc3339")]
    pub created_at: OffsetDateTime,
    pub created_by_email: String,
    #[serde(with = "time::serde::rfc3339::option")]
    pub revoked_at: Option<OffsetDateTime>,
    pub revoked_by_email: Option<String>,
}

#[derive(Serialize)]
pub struct ActiveApiKeyResponse {
    pub id: Uuid,
    pub prefix: String,
    pub status: String,
    #[serde(with = "time::serde::rfc3339")]
    pub created_at: OffsetDateTime,
}

#[derive(Serialize)]
pub struct TenantDetailResponse {
    pub id: Uuid,
    pub name: String,
    #[serde(with = "time::serde::rfc3339")]
    pub created_at: OffsetDateTime,
    pub members: Vec<MemberResponse>,
    pub api_key: Option<ActiveApiKeyResponse>,
}

pub async fn create_tenant(
    _auth: AdminAuth,
    State(state): State<Arc<AppState>>,
    Json(body): Json<CreateTenantBody>,
) -> Result<(StatusCode, Json<TenantResponse>), AdminError> {
    validate_required(&body.name, "name must not be empty")?;

    let tenant = db::admin::create_tenant(&state.db_pool, &body.name).await?;

    Ok((
        StatusCode::CREATED,
        Json(TenantResponse {
            id: tenant.id,
            name: tenant.name,
            created_at: tenant.created_at,
        }),
    ))
}

pub async fn add_member(
    _auth: AdminAuth,
    State(state): State<Arc<AppState>>,
    Path(tenant_id): Path<Uuid>,
    Json(body): Json<AddMemberBody>,
) -> Result<(StatusCode, Json<MemberResponse>), AdminError> {
    validate_required(&body.github_email, "github_email must not be empty")?;
    validate_required(&body.added_by_email, "added_by_email must not be empty")?;

    let member = db::admin::add_member(
        &state.db_pool,
        tenant_id,
        &body.github_email,
        &body.added_by_email,
    )
    .await?;

    Ok((
        StatusCode::CREATED,
        Json(MemberResponse {
            id: member.id,
            tenant_id: member.tenant_id,
            github_email: member.github_email,
            added_at: member.added_at,
        }),
    ))
}

pub async fn remove_member(
    _auth: AdminAuth,
    State(state): State<Arc<AppState>>,
    Path((tenant_id, member_id)): Path<(Uuid, Uuid)>,
    Json(body): Json<RemoveMemberBody>,
) -> Result<StatusCode, AdminError> {
    validate_required(&body.removed_by_email, "removed_by_email must not be empty")?;

    let rows_affected =
        db::admin::remove_member(&state.db_pool, tenant_id, member_id, &body.removed_by_email)
            .await?;

    if rows_affected == 0 {
        Err(AdminError::NotFound)
    } else {
        Ok(StatusCode::NO_CONTENT)
    }
}

pub async fn get_tenant(
    _auth: AdminAuth,
    State(state): State<Arc<AppState>>,
    Path(tenant_id): Path<Uuid>,
) -> Result<Json<TenantDetailResponse>, AdminError> {
    let tenant = db::admin::get_tenant(&state.db_pool, tenant_id)
        .await?
        .ok_or(AdminError::NotFound)?;

    let members = db::admin::list_members(&state.db_pool, tenant_id)
        .await?
        .into_iter()
        .map(|m| MemberResponse {
            id: m.id,
            tenant_id: m.tenant_id,
            github_email: m.github_email,
            added_at: m.added_at,
        })
        .collect();

    let api_key = db::admin::get_active_api_key(&state.db_pool, tenant_id)
        .await?
        .map(|k| ActiveApiKeyResponse {
            id: k.id,
            prefix: k.key_prefix,
            status: k.status,
            created_at: k.created_at,
        });

    Ok(Json(TenantDetailResponse {
        id: tenant.id,
        name: tenant.name,
        created_at: tenant.created_at,
        members,
        api_key,
    }))
}

pub async fn create_api_key(
    _auth: AdminAuth,
    State(state): State<Arc<AppState>>,
    Json(body): Json<CreateApiKeyBody>,
) -> Result<(StatusCode, Json<CreateApiKeyResponse>), AdminError> {
    validate_required(&body.created_by_email, "created_by_email must not be empty")?;

    let token = gen_token("wk_live_");

    let created = db::admin::create_api_key(
        &state.db_pool,
        body.tenant_id,
        &token.hash,
        &token.key_prefix,
        &body.created_by_email,
    )
    .await?;

    Ok((
        StatusCode::CREATED,
        Json(CreateApiKeyResponse {
            id: created.id,
            tenant_id: created.tenant_id,
            plaintext: token.plaintext,
            prefix: token.key_prefix,
            created_at: created.created_at,
        }),
    ))
}

pub async fn revoke_api_key(
    _auth: AdminAuth,
    State(state): State<Arc<AppState>>,
    Path(id): Path<Uuid>,
    Json(body): Json<RevokeApiKeyBody>,
) -> Result<StatusCode, AdminError> {
    validate_required(&body.revoked_by_email, "revoked_by_email must not be empty")?;

    let key_hash = db::admin::revoke_api_key(&state.db_pool, id, &body.revoked_by_email).await?;

    match key_hash {
        Some(hash) => {
            if let Ok(hash) = <[u8; 32]>::try_from(hash.as_slice()) {
                state.api_key_cache.invalidate(&hash).await;
            }
            Ok(StatusCode::NO_CONTENT)
        }
        None => Err(AdminError::NotFound),
    }
}

pub async fn list_api_keys(
    _auth: AdminAuth,
    State(state): State<Arc<AppState>>,
    Query(params): Query<ListApiKeysParams>,
) -> Result<Json<Vec<ApiKeyListItem>>, AdminError> {
    let keys = db::admin::list_api_keys(&state.db_pool, params.tenant_id)
        .await?
        .into_iter()
        .map(|k| ApiKeyListItem {
            id: k.id,
            prefix: k.key_prefix,
            status: k.status,
            created_at: k.created_at,
            created_by_email: k.created_by_email,
            revoked_at: k.revoked_at,
            revoked_by_email: k.revoked_by_email,
        })
        .collect();

    Ok(Json(keys))
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::{
        body::{Body, HttpBody},
        http::Request,
        routing::{get, post},
        Router,
    };
    use serde_json::{json, Value};
    use sqlx::PgPool;
    use std::future::poll_fn;
    use std::pin::Pin;
    use time::format_description::well_known::Rfc3339;
    use time::OffsetDateTime;
    use tower::ServiceExt;

    const TOKEN: &str = "secret-correct-token";

    async fn test_state(pool: PgPool) -> Arc<AppState> {
        let s3_client = aws_sdk_s3::Client::new(
            &aws_config::from_env()
                .region(aws_sdk_s3::config::Region::new("us-east-1"))
                .load()
                .await,
        );
        let external_class_cache =
            internal_tracing::external_class_cache::ExternalClassCache::from_env();
        let background_retry = internal_tracing::background_retry::BackgroundRetryService::new(
            pool.clone(),
            s3_client.clone(),
            external_class_cache.clone(),
        );

        Arc::new(AppState {
            db_pool: pool,
            s3_client,
            simulation_cache: crate::services::SimulationCache::new(10, 10),
            external_class_cache,
            voyager_client: None,
            background_retry,
            api_key_cache: moka::future::Cache::builder().max_capacity(10).build(),
        })
    }

    async fn protected(_auth: crate::auth::ApiKeyAuth) -> StatusCode {
        StatusCode::OK
    }

    fn build_app(state: Arc<AppState>) -> Router {
        Router::new()
            .route("/v1/admin/tenant", post(create_tenant))
            .route("/v1/admin/tenant/:tenant_id", get(get_tenant))
            .route("/v1/admin/tenant/:tenant_id/member", post(add_member))
            .route(
                "/v1/admin/tenant/:tenant_id/member/:member_id/remove",
                post(remove_member),
            )
            .route("/v1/admin/api-key", post(create_api_key).get(list_api_keys))
            .route(
                "/v1/admin/api-key/:id",
                axum::routing::delete(revoke_api_key),
            )
            .route("/protected", get(protected))
            .with_state(state)
    }

    async fn request(
        app: &Router,
        method: &str,
        uri: &str,
        token: Option<&str>,
        body: Option<Value>,
    ) -> (StatusCode, Value) {
        let mut builder = Request::builder().method(method).uri(uri);
        if let Some(t) = token {
            builder = builder.header("x-admin-token", t);
        }
        let req = match body {
            Some(v) => builder
                .header("content-type", "application/json")
                .body(Body::from(serde_json::to_vec(&v).unwrap()))
                .unwrap(),
            None => builder.body(Body::empty()).unwrap(),
        };

        let resp = app.clone().oneshot(req).await.unwrap();
        let status = resp.status();

        let mut body = resp.into_body();
        let mut bytes = Vec::new();
        while let Some(chunk) = poll_fn(|cx| Pin::new(&mut body).poll_data(cx)).await {
            bytes.extend_from_slice(chunk.unwrap().as_ref());
        }

        let json = if bytes.is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(&bytes).unwrap_or(Value::Null)
        };
        (status, json)
    }

    fn is_rfc3339(value: &Value) -> bool {
        value
            .as_str()
            .is_some_and(|s| OffsetDateTime::parse(s, &Rfc3339).is_ok())
    }

    #[sqlx::test(migrations = "./migrations")]
    async fn create_add_three_remove_one_lists_two(pool: PgPool) {
        std::env::set_var("WALNUT_ADMIN_TOKEN", TOKEN);
        let app = build_app(test_state(pool).await);

        let (status, _) = request(
            &app,
            "POST",
            "/v1/admin/tenant",
            None,
            Some(json!({ "name": "Starkware" })),
        )
        .await;
        assert_eq!(status, StatusCode::UNAUTHORIZED);

        let (status, _) = request(
            &app,
            "POST",
            "/v1/admin/tenant",
            Some(TOKEN),
            Some(json!({ "name": "" })),
        )
        .await;
        assert_eq!(status, StatusCode::BAD_REQUEST);

        let (status, body) = request(
            &app,
            "POST",
            "/v1/admin/tenant",
            Some(TOKEN),
            Some(json!({ "name": "Starkware" })),
        )
        .await;
        assert_eq!(status, StatusCode::CREATED);
        assert_eq!(body["name"], "Starkware");
        assert!(is_rfc3339(&body["created_at"]));
        let tenant_id = body["id"].as_str().unwrap().to_string();

        let mut member_ids = Vec::new();
        for email in ["a@x.co", "b@x.co", "c@x.co"] {
            let (status, body) = request(
                &app,
                "POST",
                &format!("/v1/admin/tenant/{tenant_id}/member"),
                Some(TOKEN),
                Some(json!({ "github_email": email, "added_by_email": "admin@walnut.dev" })),
            )
            .await;
            assert_eq!(status, StatusCode::CREATED);
            assert_eq!(body["github_email"], email);
            assert!(is_rfc3339(&body["added_at"]));
            member_ids.push(body["id"].as_str().unwrap().to_string());
        }

        let (status, _) = request(
            &app,
            "POST",
            &format!("/v1/admin/tenant/{tenant_id}/member"),
            Some(TOKEN),
            Some(json!({ "github_email": "a@x.co", "added_by_email": "admin@walnut.dev" })),
        )
        .await;
        assert_eq!(status, StatusCode::CONFLICT);

        let (status, _) = request(
            &app,
            "POST",
            &format!(
                "/v1/admin/tenant/{tenant_id}/member/{}/remove",
                member_ids[0]
            ),
            Some(TOKEN),
            Some(json!({ "removed_by_email": "admin@walnut.dev" })),
        )
        .await;
        assert_eq!(status, StatusCode::NO_CONTENT);

        let (status, _) = request(
            &app,
            "POST",
            &format!(
                "/v1/admin/tenant/{tenant_id}/member/{}/remove",
                member_ids[0]
            ),
            Some(TOKEN),
            Some(json!({ "removed_by_email": "admin@walnut.dev" })),
        )
        .await;
        assert_eq!(status, StatusCode::NOT_FOUND);

        let (status, body) = request(
            &app,
            "GET",
            &format!("/v1/admin/tenant/{tenant_id}"),
            Some(TOKEN),
            None,
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body["members"].as_array().unwrap().len(), 2);
        assert!(body["api_key"].is_null());

        for id in [&member_ids[1], &member_ids[2]] {
            let (status, _) = request(
                &app,
                "POST",
                &format!("/v1/admin/tenant/{tenant_id}/member/{id}/remove"),
                Some(TOKEN),
                Some(json!({ "removed_by_email": "admin@walnut.dev" })),
            )
            .await;
            assert_eq!(status, StatusCode::NO_CONTENT);
        }

        let (status, body) = request(
            &app,
            "GET",
            &format!("/v1/admin/tenant/{tenant_id}"),
            Some(TOKEN),
            None,
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body["members"].as_array().unwrap().len(), 0);
    }

    async fn create_tenant_id(app: &Router) -> String {
        let (status, body) = request(
            app,
            "POST",
            "/v1/admin/tenant",
            Some(TOKEN),
            Some(json!({ "name": "Starkware" })),
        )
        .await;
        assert_eq!(status, StatusCode::CREATED);
        body["id"].as_str().unwrap().to_string()
    }

    #[sqlx::test(migrations = "./migrations")]
    async fn api_key_lifecycle(pool: PgPool) {
        std::env::set_var("WALNUT_ADMIN_TOKEN", TOKEN);
        let app = build_app(test_state(pool).await);
        let tenant_id = create_tenant_id(&app).await;

        let (status, _) = request(
            &app,
            "POST",
            "/v1/admin/api-key",
            None,
            Some(json!({ "tenant_id": tenant_id, "created_by_email": "marija@walnut.dev" })),
        )
        .await;
        assert_eq!(status, StatusCode::UNAUTHORIZED);

        let (status, body) = request(
            &app,
            "POST",
            "/v1/admin/api-key",
            Some(TOKEN),
            Some(json!({ "tenant_id": tenant_id, "created_by_email": "marija@walnut.dev" })),
        )
        .await;
        assert_eq!(status, StatusCode::CREATED);
        let plaintext = body["plaintext"].as_str().unwrap().to_string();
        let key_id = body["id"].as_str().unwrap().to_string();
        assert!(plaintext.starts_with("wk_live_"));
        assert_eq!(body["prefix"], &plaintext[..12]);
        assert_eq!(body["tenant_id"], tenant_id);
        assert!(is_rfc3339(&body["created_at"]));

        let (status, _) = request_with_api_key(&app, &plaintext).await;
        assert_eq!(status, StatusCode::OK);

        let (status, _) = request(
            &app,
            "POST",
            "/v1/admin/api-key",
            Some(TOKEN),
            Some(json!({ "tenant_id": tenant_id, "created_by_email": "marija@walnut.dev" })),
        )
        .await;
        assert_eq!(status, StatusCode::CONFLICT);

        let (status, body) = request(
            &app,
            "GET",
            &format!("/v1/admin/tenant/{tenant_id}"),
            Some(TOKEN),
            None,
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body["api_key"]["id"], key_id);
        assert_eq!(body["api_key"]["status"], "active");
        assert!(body["api_key"].get("plaintext").is_none());

        let (status, _) = request(
            &app,
            "DELETE",
            &format!("/v1/admin/api-key/{key_id}"),
            Some(TOKEN),
            Some(json!({ "revoked_by_email": "marija@walnut.dev" })),
        )
        .await;
        assert_eq!(status, StatusCode::NO_CONTENT);

        let (status, _) = request_with_api_key(&app, &plaintext).await;
        assert_eq!(status, StatusCode::UNAUTHORIZED);

        let (status, _) = request(
            &app,
            "DELETE",
            &format!("/v1/admin/api-key/{key_id}"),
            Some(TOKEN),
            Some(json!({ "revoked_by_email": "marija@walnut.dev" })),
        )
        .await;
        assert_eq!(status, StatusCode::NOT_FOUND);

        let (status, body) = request(
            &app,
            "GET",
            &format!("/v1/admin/api-key?tenant_id={tenant_id}"),
            Some(TOKEN),
            None,
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        let keys = body.as_array().unwrap();
        assert_eq!(keys.len(), 1);
        assert_eq!(keys[0]["status"], "revoked");
        assert_eq!(keys[0]["revoked_by_email"], "marija@walnut.dev");
        assert!(is_rfc3339(&keys[0]["revoked_at"]));
        assert!(keys[0].get("plaintext").is_none());

        let (status, body) = request(
            &app,
            "POST",
            "/v1/admin/api-key",
            Some(TOKEN),
            Some(json!({ "tenant_id": tenant_id, "created_by_email": "marija@walnut.dev" })),
        )
        .await;
        assert_eq!(status, StatusCode::CREATED);
        let new_plaintext = body["plaintext"].as_str().unwrap();
        assert_ne!(new_plaintext, plaintext);

        let (status, _) = request_with_api_key(&app, new_plaintext).await;
        assert_eq!(status, StatusCode::OK);
    }

    async fn request_with_api_key(app: &Router, key: &str) -> (StatusCode, Value) {
        let req = Request::builder()
            .method("GET")
            .uri("/protected")
            .header("x-api-key", key)
            .body(Body::empty())
            .unwrap();
        let resp = app.clone().oneshot(req).await.unwrap();
        (resp.status(), Value::Null)
    }
}
