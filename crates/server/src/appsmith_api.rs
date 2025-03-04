use std::sync::Arc;
use axum::extract::State;
use axum::http::HeaderMap;
use axum::Json;
use axum::response::{IntoResponse, Response};
use reqwest::StatusCode;
use serde_json::json;
use sqlx::{Pool, Postgres};
use tracing::error;
use crate::app_state::AppState;

pub async fn get_verification_data(
        State(state): State<Arc<AppState>>,
        headers: HeaderMap) -> Response {
    let auth_header = headers.get("Authorization");
    // some simple security protection
    if auth_header.is_none() || auth_header.unwrap().to_str().unwrap_or("") != "a3250f90-f974-498f-a2b0-371fa6b0db35" {
        return StatusCode::UNAUTHORIZED.into_response();
    }
    let total_failed_count = query_count(&state.db_pool, "failed").await.unwrap_or(0);
    let total_success_count = query_count(&state.db_pool, "success").await.unwrap_or(0);
    let total_pending_count = query_count(&state.db_pool, "pending").await.unwrap_or(0);

    let last_30_days_verifications = sqlx::query!(
        r#"SELECT
            date_trunc('day', created_at) AS date,
                status,
                COUNT(*) AS status_count
            FROM
                public.verification_status
            WHERE
                created_at >= NOW() - INTERVAL '30 days'
            GROUP BY
                date, status
            ORDER BY
                date ASC, status;"#
    ).fetch_all(&state.db_pool).await.unwrap_or_else(|_| {
        error!("Failed to fetch last 30 days verifications");
        vec![]
    });

    let failed_verifications = sqlx::query!(
        r#"SELECT * FROM public.verification_status WHERE status = 'failed' ORDER BY created_at DESC LIMIT 20;"#
    ).fetch_all(&state.db_pool).await.unwrap_or_else(|_| {
        error!("Failed to fetch failed verifications");
        vec![]
    });

    let json_response = json!({
        "last_30_days": last_30_days_verifications.iter().map(|row| json!({
            "date": row.date.unwrap().to_string(),
            "status": row.status,
            "count": row.status_count
        })).collect::<Vec<_>>(),

        "failed_verifications": failed_verifications.iter().map(|row| json!({
            "id": row.id,
            "status": row.status,
            "created_at": row.created_at.to_string(),
            "class_hash": row.class_hash,
            "message": row.message,
            "updated_at": row.updated_at.to_string(),
            "primary_id": row.primary_id.to_string(),
        })).collect::<Vec<_>>(),

        "total_failed_count": total_failed_count,
        "total_success_count": total_success_count,
        "total_pending_count": total_pending_count,
    });

    (StatusCode::OK, Json(json_response)).into_response()
}

async fn query_count(pool: &Pool<Postgres>, status: &str) -> Result<i64, sqlx::Error> {
    let count = sqlx::query!(
            r#"SELECT COUNT(*) as count FROM public.verification_status WHERE status = $1"#,
            status
        )
        .fetch_one(pool)
        .await?
        .count
        .unwrap_or_else(|| {
            error!("Failed to fetch verification status count for status {}.", status);
            0
        });
    Ok(count)
}
