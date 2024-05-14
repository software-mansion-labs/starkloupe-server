use crate::app_state::AppState;
use axum::{
    extract::{Path, State},
    http::StatusCode,
    Json,
};
use blockifier::transaction::objects::TransactionExecutionInfo;
use cheatnet::state::TraceData;
use db::Simulation;
use serde::Serialize;
use simulate::{
    simulate, simulate_transaction_by_hash, SimulationArgs, SimulationInfo, SimulationRawArgs,
    TransactionSimulationResult,
};
use sqlx::types::Uuid;
use starknet::core::types::{
    CallType, ExecuteInvocation, FeeEstimate, FieldElement, FunctionInvocation, InvokeTransaction,
    InvokeTransactionTrace, SimulatedTransaction, Transaction, TransactionTrace,
};
use starknet_api::{
    contract_address,
    core::{ContractAddress, Nonce},
    hash::{StarkFelt, StarkHash},
};
use starknet_api::{
    core::{ChainId, PatriciaKey},
    transaction::Calldata,
};
use starknet_api::{patricia_key, stark_felt};
use starknet_providers::Provider;
use std::{str::FromStr, sync::Arc};
use walnut_shared::{create_rpc_client, extract_chain_id};

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

    // let simulation_info = simulate(SimulationArgs {
    //     chain_id: sim.chain_id.clone(),
    //     block_at: (sim.block_at as u64).clone(),
    //     nonce: (sim.nonce as u64).clone(),
    //     wallet_address: sim.wallet_address.clone(),
    //     calldata: sim.calldata.clone().unwrap_or_default(),
    // });

    Err(StatusCode::EXPECTATION_FAILED)
    // match tx_info {
    //     Ok(tx_info) => Ok(Json(SimulateTraceResponse {
    //         simulated_transaction: to_simulated_transaction(tx_info),
    //         simulation: Some(sim),
    //     })),
    //     Err(_) => Err(StatusCode::EXPECTATION_FAILED),
    // }
}

pub async fn simulate_transaction(
    Json(payload): Json<SimulationRawArgs>,
) -> Result<Json<SimulationInfo>, StatusCode> {
    let simulation_info = simulate(payload.into()).await;
    Ok(Json(simulation_info))
}

pub async fn simulate_transaction_by_hash_handler(
    Path((chain_id, tx_hash)): Path<(String, String)>,
) -> Result<Json<TransactionSimulationResult>, StatusCode> {
    let chain_id = extract_chain_id(chain_id.as_str());
    let simulation_info = simulate_transaction_by_hash(chain_id, tx_hash).await;
    Ok(Json(simulation_info.unwrap()))
}
