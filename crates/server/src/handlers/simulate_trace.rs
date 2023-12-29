use crate::app_state::AppState;
use axum::{
    extract::{Path, State},
    http::StatusCode,
    Json,
};
use blockifier::transaction::objects::TransactionExecutionInfo;
use db::Simulation;
use serde::Serialize;
use simulate::{simulate, to_simulated_transaction, SimulationArgs};
use sqlx::types::Uuid;
use starknet::core::types::{
    CallType, EventContent, ExecuteInvocation, FeeEstimate, FieldElement, FunctionInvocation,
    InvokeTransactionTrace, SimulatedTransaction, TransactionTrace,
};
use starknet_api::hash::StarkFelt;
use std::{str::FromStr, sync::Arc};

#[derive(Serialize)]
pub struct SimulateTraceResponse {
    simulated_transaction: SimulatedTransaction,
    simulation: Option<Simulation>,
}

pub async fn simulate_trace(
    State(state): State<Arc<AppState>>,
    id: Path<String>,
) -> Result<Json<SimulateTraceResponse>, StatusCode> {
    // Implement your business logic here
    let sim: Simulation = sqlx::query_as!(
        Simulation,
        "SELECT * FROM simulations WHERE id = $1",
        Uuid::from_str(&id.0).unwrap()
    )
    .fetch_one(&state.db_pool)
    .await
    .unwrap();

    let tx_info = simulate(SimulationArgs {
        chain_id: sim.chain_id.clone(),
        block_at: (sim.block_at as u64).clone(),
        nonce: (sim.nonce as u64).clone(),
        wallet_address: sim.wallet_address.clone(),
        calldata: sim.calldata.clone().unwrap_or_default(),
    });

    match tx_info {
        Ok(tx_info) => Ok(Json(SimulateTraceResponse {
            simulated_transaction: to_simulated_transaction(tx_info),
            simulation: Some(sim),
        })),
        Err(_) => Err(StatusCode::EXPECTATION_FAILED),
    }
}

pub async fn simulate_transaction(
    Json(payload): Json<SimulationArgs>,
) -> Result<Json<SimulateTraceResponse>, StatusCode> {
    let tx_info = simulate(payload);

    match tx_info {
        Ok(tx_info) => Ok(Json(SimulateTraceResponse {
            simulated_transaction: to_simulated_transaction(tx_info),
            simulation: None,
        })),
        Err(e) => {
            dbg!(e);
            Err(StatusCode::EXPECTATION_FAILED)
        }
    }
}
