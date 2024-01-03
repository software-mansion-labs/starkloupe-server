use crate::app_state::AppState;
use axum::extract::Query;
use axum::Extension;
use axum::{extract::State, http::StatusCode, Json};
use db::{Project, User};
use serde::{Deserialize, Serialize};
use std::sync::Arc;

use super::auth::validate_project;

#[derive(Serialize, Deserialize)]
pub struct SimulationsRequest {
    project_slug: Option<String>,
}

#[derive(Serialize)]
pub struct CommonError {
    error_message: String,
    error_count: i64,
}

#[derive(Serialize)]
pub struct CommonErrorsResponse {
    common_errors: Vec<CommonError>,
    project: Project,
}

pub async fn get_common_errors(
    Extension(user_projects): Extension<Vec<Project>>,
    Extension(user): Extension<User>,
    State(state): State<Arc<AppState>>,
    Query(query): Query<SimulationsRequest>,
) -> Result<Json<Option<CommonErrorsResponse>>, StatusCode> {
    let project = validate_project(&state.db_pool, user, user_projects, query.project_slug).await?;

    let common_errors = match sqlx::query!(
        r#"
            SELECT
                error_message,
                COUNT(*) as error_count
            FROM simulations
            WHERE project_id = $1 AND created_at >= NOW() - INTERVAL '7 days'
            GROUP BY error_message
            ORDER BY error_count DESC
            "#,
        project.id
    )
    .fetch_all(&state.db_pool)
    .await
    {
        Ok(rows) => rows
            .iter()
            .map(|row| CommonError {
                error_message: row.error_message.clone().unwrap_or_default(),
                error_count: row.error_count.unwrap_or(0),
            })
            .collect(),
        Err(_) => vec![],
    };

    Ok(Json(Some(CommonErrorsResponse {
        common_errors,
        project,
    })))
}
