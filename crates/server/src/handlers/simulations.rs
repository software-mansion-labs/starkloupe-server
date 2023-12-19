use crate::app_state::AppState;
use axum::extract::Query;
use axum::Extension;
use axum::{extract::State, http::StatusCode, Json};
use db::Project;
use serde::{Deserialize, Serialize};
use simulate::SimulationRes;
use std::sync::Arc;

#[derive(Serialize, Deserialize)]
pub struct SimulationsRequest {
    wallet_address: Option<String>,
    project_id: Option<i32>,
}

#[derive(Serialize)]
pub struct StatsResponse {
    failure_simulations: i64,
    total_simulations: i64,
    unique_wallet_count: i64,
}

#[derive(Serialize)]
pub struct SimulationsResponse {
    simulations: Vec<SimulationRes>,
    stats: StatsResponse,
    project: Project,
}

pub async fn get_simulations(
    Extension(projects): Extension<Vec<Project>>,
    State(state): State<Arc<AppState>>,
    Query(query): Query<SimulationsRequest>,
) -> Result<Json<Option<SimulationsResponse>>, StatusCode> {
    let project = match projects.first() {
        Some(project) => project,
        None => return Ok(Json(None)),
    };

    let simulations = match sqlx::query!(
        "SELECT * FROM simulations WHERE project_id = $1 ORDER BY created_at DESC",
        project.id
    )
    .fetch_all(&state.db_pool)
    .await
    {
        Ok(simulations) => simulations,
        Err(_) => Vec::new(),
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

    let simulations_res: Vec<SimulationRes> = simulations
        .into_iter()
        .filter(|simulation| {
            if query.project_id.is_some() && query.wallet_address.is_some() {
                simulation.project_id == query.project_id.unwrap()
                    && simulation.wallet_address == query.wallet_address.clone().unwrap()
            } else if query.project_id.is_some() {
                simulation.project_id == query.project_id.unwrap()
            } else if query.wallet_address.is_some() {
                simulation.wallet_address == query.wallet_address.clone().unwrap()
            } else {
                true
            }
        })
        .map(|simulation| SimulationRes {
            id: simulation.id.map_or(String::new(), |id| id.to_string()),
            project_id: simulation.project_id,
            chain_id: simulation.chain_id,
            block_at: simulation.block_at,
            transaction_version: simulation.transaction_version,
            nonce: simulation.nonce,
            max_fee: simulation.max_fee,
            cairo_version: simulation.cairo_version,
            wallet_address: simulation.wallet_address,
            calldata: simulation.calldata.map_or(Vec::new(), |calldata| calldata),
            created_at: simulation.created_at.assume_utc().unix_timestamp(),
            updated_at: simulation.updated_at.assume_utc().unix_timestamp(),
            status: simulation.status,
        })
        .collect();

    Ok(Json(Some(SimulationsResponse {
        simulations: simulations_res,
        stats: StatsResponse {
            failure_simulations: simulations_with_failure_count,
            total_simulations: total_simulations_count,
            unique_wallet_count,
        },
        project: project.clone(),
    })))
}
