use crate::app_state::AppState;
use axum::extract::Query;
use axum::Extension;
use axum::{extract::State, http::StatusCode, Json};
use db::{Project, Simulation, User};
use serde::{Deserialize, Serialize};
use std::sync::Arc;

#[derive(Serialize, Deserialize)]
pub struct SimulationsRequest {
    wallet_address: Option<String>,
    project_slug: Option<String>,
}

#[derive(Serialize)]
pub struct StatsResponse {
    failure_simulations: i64,
    total_simulations: i64,
    unique_wallet_count: i64,
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
    let project: Project = if let Some(project_slug) = query.project_slug.clone() {
        let project = user_projects
            .iter()
            .find(|p| p.slug == project_slug)
            .cloned();
        match project {
            Some(project) => project,
            None => {
                if user.email.ends_with("@walnut.dev") {
                    // Admin access
                    let project_slug = query.project_slug.clone().unwrap_or_default();
                    // TODO: cache projects separately and use cache here
                    match sqlx::query!("SELECT * FROM projects WHERE slug = $1", project_slug)
                        .fetch_one(&state.db_pool)
                        .await
                    {
                        Ok(row) => Project {
                            id: row.id,
                            name: row.name,
                            slug: row.slug,
                        },
                        Err(_) => {
                            // Poject not found
                            return Err(StatusCode::NOT_FOUND);
                        }
                    }
                } else {
                    // Project not found and no admin access
                    return Err(StatusCode::NOT_FOUND);
                }
            }
        }
    } else {
        // No project slug in query
        let project = user_projects.first().cloned();
        if let Some(project) = project {
            project
        } else {
            // No projects linked to user
            return Err(StatusCode::NOT_FOUND);
        }
    };

    let simulations = match sqlx::query_as!(
        Simulation,
        "SELECT simulations.* FROM simulations WHERE project_id = $1 ORDER BY created_at DESC;",
        project.id
    )
    .fetch_all(&state.db_pool)
    .await
    {
        Ok(rows) => rows,
        Err(_) => {
            return Err(StatusCode::INTERNAL_SERVER_ERROR);
        }
    };

    let (simulations_with_failure_count, total_simulations_count, unique_wallet_count) =
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
            Ok(row) => (
                row.failure_count.unwrap_or(0),
                row.total_count.unwrap_or(0),
                row.unique_wallet_count.unwrap_or(0),
            ),
            Err(_) => (0, 0, 0),
        };

    Ok(Json(Some(SimulationsResponse {
        simulations,
        stats: StatsResponse {
            failure_simulations: simulations_with_failure_count,
            total_simulations: total_simulations_count,
            unique_wallet_count,
        },
        project,
    })))
}
