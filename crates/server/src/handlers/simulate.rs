use crate::app_state::AppState;
use crate::config::rpc_url;
use axum::{extract::State, http::StatusCode, Extension, Json};
use blockifier::transaction::objects::TransactionExecutionInfo;
use serde::{Deserialize, Serialize};
use simulate::{simulate, SimulationArgs, SimulationRawArgs};
use sqlx::types::Uuid;
use starknet::core::types::{
    BlockId, ExecuteInvocation, FieldElement, FunctionInvocation, TransactionTrace,
};
use starknet_providers::{
    jsonrpc::{HttpTransport, JsonRpcClient},
    Provider,
};
use std::sync::Arc;
use tracing::info;
use url::Url;

#[derive(Serialize, Deserialize)]
pub struct StarkNetTransaction {
    chain_id: String,
    wallet_address: String,
    calldata: Vec<String>,
    nonce: Option<u8>,
    max_fee: Option<u128>,
    cairo_version: String,
}

#[derive(Serialize)]
pub struct SimulateResult {}

pub async fn simulate_handler(
    Extension(project): Extension<db::Project>,
    State(state): State<Arc<AppState>>,
    Json(payload): Json<StarkNetTransaction>,
) -> Result<Json<SimulateResult>, StatusCode> {
    let rpc_client = create_rpc_client(payload.chain_id.clone());

    let block_number = rpc_client.block_number().await.unwrap();

    let mut sim = db::Simulation::default();
    sim.project_id = project.id;
    sim.chain_id = payload.chain_id;
    sim.block_at = block_number as i32;
    sim.transaction_version = 0;
    sim.max_fee = match payload.max_fee {
        Some(max_fee) => max_fee.to_string(),
        None => String::from(""),
    };
    sim.cairo_version = payload.cairo_version;
    sim.wallet_address = payload.wallet_address;
    sim.calldata = Some(payload.calldata);
    sim.nonce = u32::try_from(
        rpc_client
            .get_nonce(
                BlockId::Number(block_number),
                FieldElement::from_hex_be(sim.wallet_address.as_str()).unwrap(),
            )
            .await
            .unwrap(),
    )
    .unwrap() as i32;

    // Insert into database
    let row: (Uuid,) = sqlx::query_as(
        "INSERT INTO simulations (project_id, chain_id, block_at, transaction_version, nonce, max_fee, cairo_version, wallet_address, calldata) VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9) RETURNING id")
        .bind(&sim.project_id)
        .bind(&sim.chain_id)
        .bind(&sim.block_at)
        .bind(&sim.transaction_version)
        .bind(&sim.nonce)
        .bind(&sim.max_fee)
        .bind(&sim.cairo_version)
        .bind(&sim.wallet_address)
        .bind(&sim.calldata)
    .fetch_one(&state.db_pool).await.unwrap();

    let id = row.0;
    info!("Inserted into database with id {}", id);

    let args = SimulationRawArgs {
        chain_id: sim.chain_id.clone(),
        block_number: (sim.block_at as u64).clone(),
        nonce: (sim.nonce as u64).clone(),
        sender_address: sim.wallet_address.clone(),
        calldata: sim.calldata.clone().unwrap_or_default(),
    };
    let tx_info = simulate(args.into());

    let mut error_message: Option<String> = None;
    let mut error_contract_address: Option<String> = None;

    let sim_status = "success";
    // let sim_status = match tx_info {
    //     Ok(tx_info) => {
    //         if tx_info.revert_error.is_some() {
    //             let transaction_trace = to_simulated_transaction(tx_info).transaction_trace;
    //             match transaction_trace {
    //                 TransactionTrace::Invoke(transaction_trace) => {
    //                     match transaction_trace.execute_invocation {
    //                         ExecuteInvocation::Success(function_invocation) => {
    //                             (error_message, error_contract_address) =
    //                                 get_error_from_call(function_invocation);
    //                         }
    //                         _ => {}
    //                     }
    //                 }
    //                 _ => {}
    //             }
    //             "failure"
    //         } else {
    //             "success"
    //         }
    //     }

    //     Err(err) => {
    //         error_message = Some(err.to_string());
    //         "failure"
    //     }
    // };

    sqlx::query!(
        "UPDATE simulations SET status = $1, error_message = $2, error_contract_address = $3 WHERE id = $4",
        sim_status,
        error_message,
        error_contract_address,
        id,
    )
    .execute(&state.db_pool)
    .await
    .unwrap();

    Ok(Json(SimulateResult {}))
}

fn create_rpc_client(chain_id: String) -> JsonRpcClient<HttpTransport> {
    JsonRpcClient::new(HttpTransport::new(Url::parse(rpc_url(&chain_id)).unwrap()))
}
