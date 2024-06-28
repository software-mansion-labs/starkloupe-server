use crate::app_state::AppState;
use axum::{
    debug_handler,
    extract::{Path, State},
    http::StatusCode,
    Json,
};
use db::Simulation;
use serde::Serialize;
use simulate::{
    simulate_by_data, simulate_transaction_by_hash, SimulationRawArgs, TransactionSimulationResult,
};
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
) -> Result<Json<TransactionSimulationResult>, StatusCode> {
    let simulation_info = simulate_by_data(&state.db_pool, &state.s3_client, payload.into()).await;
    Ok(Json(simulation_info))
}

pub async fn simulate_transaction_by_hash_handler(
    State(state): State<Arc<AppState>>,
    Path((chain_id, tx_hash)): Path<(String, String)>,
) -> Result<Json<TransactionSimulationResult>, StatusCode> {
    let chain_id = extract_chain_id(chain_id.as_str());
    let simulation_info =
        simulate_transaction_by_hash(&state.db_pool, &state.s3_client, chain_id, tx_hash).await;
    Ok(Json(simulation_info.unwrap()))
}
