use crate::app_state::AppState;
use crate::auth::admin_token::AdminAuth;
use axum::{
    extract::{Path, State},
    http::StatusCode,
    Json,
};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use time::format_description::well_known::Rfc3339;
use uuid::Uuid;

/// Format a `TIMESTAMPTZ` value as an RFC 3339 string for JSON responses.
fn to_rfc3339(ts: time::OffsetDateTime) -> Result<String, StatusCode> {
    ts.format(&Rfc3339).map_err(|e| {
        tracing::error!("timestamp format failed: {}", e);
        StatusCode::INTERNAL_SERVER_ERROR
    })
}

#[derive(Deserialize)]
pub struct CreateTenantBody {
    pub name: String,
}

#[derive(Serialize)]
pub struct TenantResponse {
    pub id: Uuid,
    pub name: String,
    pub created_at: String,
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
    pub added_at: String,
}

#[derive(Deserialize)]
pub struct RemoveMemberBody {
    pub removed_by_email: String,
}

pub async fn create_tenant(
    _auth: AdminAuth,
    State(state): State<Arc<AppState>>,
    Json(body): Json<CreateTenantBody>,
) -> Result<(StatusCode, Json<TenantResponse>), StatusCode> {
    let row = sqlx::query!(
        "INSERT INTO tenants (name) VALUES ($1) RETURNING id, name, created_at",
        body.name
    )
    .fetch_one(&state.db_pool)
    .await
    .map_err(|e| {
        tracing::error!("create_tenant failed: {}", e);
        StatusCode::INTERNAL_SERVER_ERROR
    })?;

    Ok((
        StatusCode::CREATED,
        Json(TenantResponse {
            id: row.id,
            name: row.name,
            created_at: to_rfc3339(row.created_at)?,
        }),
    ))
}

pub async fn add_member(
    _auth: AdminAuth,
    State(state): State<Arc<AppState>>,
    Path(tenant_id): Path<Uuid>,
    Json(body): Json<AddMemberBody>,
) -> Result<(StatusCode, Json<MemberResponse>), StatusCode> {
    let row = sqlx::query!(
        "INSERT INTO tenant_members (tenant_id, github_email, added_by_email) \
         VALUES ($1, $2, $3) \
         RETURNING id, tenant_id, github_email, added_at",
        tenant_id,
        body.github_email,
        body.added_by_email
    )
    .fetch_one(&state.db_pool)
    .await
    .map_err(|e| match &e {
        sqlx::Error::Database(db) if db.is_unique_violation() => StatusCode::CONFLICT,
        _ => {
            tracing::error!("add_member insert failed: {}", e);
            StatusCode::INTERNAL_SERVER_ERROR
        }
    })?;

    Ok((
        StatusCode::CREATED,
        Json(MemberResponse {
            id: row.id,
            tenant_id: row.tenant_id,
            github_email: row.github_email,
            added_at: to_rfc3339(row.added_at)?,
        }),
    ))
}

pub async fn remove_member(
    _auth: AdminAuth,
    State(state): State<Arc<AppState>>,
    Path((tenant_id, member_id)): Path<(Uuid, Uuid)>,
    Json(body): Json<RemoveMemberBody>,
) -> Result<StatusCode, StatusCode> {
    let result = sqlx::query!(
        "UPDATE tenant_members \
         SET removed_at = now(), removed_by_email = $1 \
         WHERE id = $2 AND tenant_id = $3 AND removed_at IS NULL",
        body.removed_by_email,
        member_id,
        tenant_id
    )
    .execute(&state.db_pool)
    .await
    .map_err(|e| {
        tracing::error!("remove_member failed: {}", e);
        StatusCode::INTERNAL_SERVER_ERROR
    })?;

    if result.rows_affected() == 0 {
        Err(StatusCode::NOT_FOUND)
    } else {
        Ok(StatusCode::NO_CONTENT)
    }
}

pub async fn list_members(
    _auth: AdminAuth,
    State(state): State<Arc<AppState>>,
    Path(tenant_id): Path<Uuid>,
) -> Result<Json<Vec<MemberResponse>>, StatusCode> {
    let rows = sqlx::query!(
        "SELECT id, tenant_id, github_email, added_at FROM tenant_members \
         WHERE tenant_id = $1 AND removed_at IS NULL \
         ORDER BY added_at",
        tenant_id
    )
    .fetch_all(&state.db_pool)
    .await
    .map_err(|e| {
        tracing::error!("list_members failed: {}", e);
        StatusCode::INTERNAL_SERVER_ERROR
    })?;

    let members = rows
        .into_iter()
        .map(|r| {
            Ok(MemberResponse {
                id: r.id,
                tenant_id: r.tenant_id,
                github_email: r.github_email,
                added_at: to_rfc3339(r.added_at)?,
            })
        })
        .collect::<Result<Vec<_>, StatusCode>>()?;

    Ok(Json(members))
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::{
        body::{Body, HttpBody},
        http::Request,
        routing::{delete, get, post},
        Router,
    };
    use serde_json::{json, Value};
    use sqlx::PgPool;
    use std::future::poll_fn;
    use std::pin::Pin;
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

    fn build_app(state: Arc<AppState>) -> Router {
        Router::new()
            .route("/v1/admin/tenant", post(create_tenant))
            .route("/v1/admin/tenant/:tenant_id", get(list_members))
            .route("/v1/admin/tenant/:tenant_id/member", post(add_member))
            .route(
                "/v1/admin/tenant/:tenant_id/member/:member_id",
                delete(remove_member),
            )
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
            "DELETE",
            &format!("/v1/admin/tenant/{tenant_id}/member/{}", member_ids[0]),
            Some(TOKEN),
            Some(json!({ "removed_by_email": "admin@walnut.dev" })),
        )
        .await;
        assert_eq!(status, StatusCode::NO_CONTENT);

        let (status, _) = request(
            &app,
            "DELETE",
            &format!("/v1/admin/tenant/{tenant_id}/member/{}", member_ids[0]),
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
        assert_eq!(body.as_array().unwrap().len(), 2);
    }
}
