use crate::app_state::AppState;
use axum::extract::Query;
use axum::Extension;
use axum::{extract::State, http::StatusCode, Json};
use db::{Project, Simulation, User};
use futures::future;
use serde::{Deserialize, Serialize};
use std::sync::Arc;

use super::auth::validate_project;

#[derive(Serialize, Deserialize)]
pub struct SimulationsRequest {
    error_hash: Option<String>,
    project_slug: Option<String>,
}

#[derive(Serialize)]
pub struct CommonError {
    error_message: String,
    error_count: i64,
}

#[derive(Serialize)]
pub struct StatsResponse {
    failure_simulations: i64,
    total_simulations: i64,
    unique_wallet_count: i64,
    common_errors: Vec<CommonError>,
}

#[derive(Serialize)]
pub struct SimulationsResponse {
    simulations: Vec<Simulation>,
    stats: StatsResponse,
    project: Project,
}

pub async fn get_simulations(
    Extension(user_projects): Extension<Vec<Project>>,
    Extension(user): Extension<User>,
    State(state): State<Arc<AppState>>,
    Query(query): Query<SimulationsRequest>,
) -> Result<Json<Option<SimulationsResponse>>, StatusCode> {
    let project = validate_project(&state.db_pool, user, user_projects, query.project_slug).await?;

    let simulations_future = async {
        match query.error_hash {
            Some(error_hash) => {
                match sqlx::query_as!(
                    Simulation,
                    "SELECT simulations.* FROM simulations WHERE project_id = $1 AND md5(error_message) = $2 ORDER BY created_at DESC;",
                    project.id,
                    error_hash
                )
                .fetch_all(&state.db_pool)
                .await {
                    Ok(simulations) => Ok(simulations),
                    Err(_) => Err(StatusCode::INTERNAL_SERVER_ERROR),
                }
            }
            None => {
                match sqlx::query_as!(
                    Simulation,
                    "SELECT simulations.* FROM simulations WHERE project_id = $1 ORDER BY created_at DESC;",
                    project.id
                )
                .fetch_all(&state.db_pool)
                .await {
                    Ok(simulations) => Ok(simulations),
                    Err(_) => Err(StatusCode::INTERNAL_SERVER_ERROR),
                }
            }
        }
    };

    let stats_future = async {
        match sqlx::query!(
            r#"
                SELECT
                    SUM(CASE WHEN status = 'failure' THEN 1 ELSE 0 END) as failure_count,
                    COUNT(*) as total_count,
                    COUNT(DISTINCT wallet_address) as unique_wallet_count
                FROM simulations
                WHERE project_id = $1 AND created_at >= NOW() - INTERVAL '7 days'
                "#,
            project.id
        )
        .fetch_one(&state.db_pool)
        .await
        {
            Ok(row) => Ok((
                row.failure_count.unwrap_or(0),
                row.total_count.unwrap_or(0),
                row.unique_wallet_count.unwrap_or(0),
            )),
            Err(_) => Err(StatusCode::INTERNAL_SERVER_ERROR),
        }
    };

    let common_errors_future = async {
        match sqlx::query!(
            r#"
                    SELECT
                    error_message,
                    COUNT(*) as error_count
                FROM simulations
                WHERE project_id = $1 AND created_at >= NOW() - INTERVAL '7 days'
                GROUP BY error_message
                ORDER BY error_count DESC
                LIMIT 5
                "#,
            project.id
        )
        .fetch_all(&state.db_pool)
        .await
        {
            Ok(rows) => Ok(rows
                .into_iter()
                .map(|row| CommonError {
                    error_message: row.error_message.clone().unwrap_or_default(),
                    error_count: row.error_count.unwrap_or(0),
                })
                .collect()),
            Err(_) => Err(StatusCode::INTERNAL_SERVER_ERROR),
        }
    };

    let result = future::try_join3(simulations_future, stats_future, common_errors_future).await;

    match result {
        Ok((
            simulations,
            (simulations_with_failure_count, total_simulations_count, unique_wallet_count),
            common_errors,
        )) => Ok(Json(Some(SimulationsResponse {
            simulations,
            stats: StatsResponse {
                failure_simulations: simulations_with_failure_count,
                total_simulations: total_simulations_count,
                unique_wallet_count,
                common_errors,
            },
            project,
        }))),
        Err(_) => Err(StatusCode::INTERNAL_SERVER_ERROR),
    }
}
