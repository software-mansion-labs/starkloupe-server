use crate::app_state::AppState;
use axum::extract::Query;
use axum::{extract::State, http::StatusCode, Json};
use serde::{Deserialize, Serialize};
use std::sync::Arc;

#[derive(Serialize, Deserialize)]
pub struct SimulationsRequest {
    wallet_address: Option<String>,
    team_id: Option<i32>,
}

#[derive(Serialize)]
pub struct SimulationRes {
    pub id: String,
    pub team_id: i32,
    pub chain_id: String,
    pub block_at: i32,
    pub transaction_version: i32,
    pub nonce: i32,
    pub max_fee: String,
    pub cairo_version: String,
    pub wallet_address: String,
    pub calldata: Vec<String>,
    pub created_at: i64,
    pub updated_at: i64,
    pub status: String,
}

#[derive(Serialize)]
pub struct SimulationsResponse {
    simulations: Vec<SimulationRes>,
}

pub async fn get_simulations(
    State(state): State<Arc<AppState>>,
    Query(query): Query<SimulationsRequest>,
) -> Result<Json<SimulationsResponse>, StatusCode> {
    let simulations = match sqlx::query!("SELECT * FROM simulations order by created_at DESC")
        .fetch_all(&state.db_pool)
        .await
    {
        Ok(value) => value,
        Err(_other_error) => Vec::new(),
    };

    let simulations_res: Vec<SimulationRes> = simulations
        .into_iter()
        .filter(|simulation| {
            if query.team_id.is_some() && query.wallet_address.is_some() {
                simulation.team_id == query.team_id.unwrap()
                    && simulation.wallet_address == query.wallet_address.clone().unwrap()
            } else if query.team_id.is_some() {
                simulation.team_id == query.team_id.unwrap()
            } else if query.wallet_address.is_some() {
                simulation.wallet_address == query.wallet_address.clone().unwrap()
            } else {
                true
            }
        })
        .map(|simulation| SimulationRes {
            id: simulation.id.map_or(String::new(), |id| id.to_string()),
            team_id: simulation.team_id,
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

    Ok(Json(SimulationsResponse {
        simulations: simulations_res,
    }))
}
