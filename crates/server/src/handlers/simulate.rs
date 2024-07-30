use crate::app_state::AppState;
use axum::{
    debug_handler,
    extract::{Path, State},
    http::StatusCode,
    response::{IntoResponse, Response},
    Json,
};
use db::Simulation;
use serde::Serialize;
use simulate::{simulate_by_data, simulate_transaction_by_hash, SimulationRawArgs};
use starknet::core::types::SimulatedTransaction;
use std::sync::Arc;
use walnut_shared::extract_chain_id;

#[derive(Serialize)]
pub struct SimulateTraceResponse {
    simulated_transaction: SimulatedTransaction,
    simulation: Option<Simulation>,
}

#[debug_handler]
pub async fn simulate_transaction(
    State(state): State<Arc<AppState>>,
    Json(payload): Json<SimulationRawArgs>,
) -> Response {
    let simulation_info = simulate_by_data(&state.db_pool, &state.s3_client, payload.into()).await;
    match simulation_info {
        Ok(simulation_info) => (StatusCode::OK, Json(simulation_info)).into_response(),
        Err(e) => (StatusCode::BAD_REQUEST, Json(e.to_string())).into_response(),
    }
}

pub async fn simulate_transaction_by_hash_handler(
    State(state): State<Arc<AppState>>,
    Path((chain_id, tx_hash)): Path<(String, String)>,
) -> Response {
    let chain_id = extract_chain_id(chain_id.as_str());
    let simulation_info =
        simulate_transaction_by_hash(&state.db_pool, &state.s3_client, chain_id, tx_hash).await;
    match simulation_info {
        Ok(simulation_info) => (StatusCode::OK, Json(simulation_info)).into_response(),
        Err(e) => (StatusCode::BAD_REQUEST, Json(e.to_string())).into_response(),
    }
}
