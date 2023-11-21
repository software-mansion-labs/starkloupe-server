use crate::app_state::AppState;
use crate::handlers::simulations::SimulationRes;
use axum::{
    extract::{Path, State},
    http::StatusCode,
    Json,
};
use serde::Serialize;
use sqlx::types::Uuid;
use starknet::core::types::{
    BlockId, BroadcastedInvokeTransaction, BroadcastedTransaction, FieldElement,
    SimulatedTransaction, SimulationFlag,
};
use starknet_providers::{
    jsonrpc::{HttpTransport, JsonRpcClient},
    Provider,
};
use std::{str::FromStr, sync::Arc};
use url::Url;

pub fn convert_array(arr: Vec<String>) -> Vec<String> {
    // Convert String to u64
    let num_transactions = arr[0].clone().parse::<i32>().unwrap();
    let mut converted_arr: Vec<String> = Vec::new();
    converted_arr.push(arr[0].clone());

    for i in 0..num_transactions {
        let contract_address = arr[4 * i as usize + 1].clone();
        let selector = arr[4 * i as usize + 2].clone();
        converted_arr.push(contract_address);
        converted_arr.push(selector);

        let calldata_len_current = arr[4 * i as usize + 4].clone().parse::<i32>().unwrap();
        converted_arr.push(arr[4 * i as usize + 4].clone());

        let start_index: usize = 4 * num_transactions as usize
            + 2
            + arr[4 * i as usize + 3].parse::<i32>().unwrap() as usize;
        let end_index: usize = start_index + calldata_len_current as usize;

        let args = &arr[start_index..end_index];
        converted_arr.extend_from_slice(args);
    }
    converted_arr
}

fn create_rpc_client(chain_id: String) -> JsonRpcClient<HttpTransport> {
    let url = match chain_id.as_str() {
        "0x534e5f474f45524c49" => "https://3dfa-54-87-10-131.ngrok-free.app",
        "0x534e5f4d41494e" => "https://0721-54-87-10-131.ngrok-free.app",
        _ => panic!("Invalid chain id"),
    };
    JsonRpcClient::new(HttpTransport::new(Url::parse(url).unwrap()))
}

#[derive(Serialize)]
pub struct SimulateTraceResponse {
    simulated_transaction: SimulatedTransaction,
    simulation: SimulationRes,
}

pub async fn simulate_trace(
    State(state): State<Arc<AppState>>,
    id: Path<String>,
) -> Result<Json<SimulateTraceResponse>, StatusCode> {
    // Implement your business logic here
    let row = sqlx::query!(
        "SELECT * FROM simulations WHERE id = $1",
        Uuid::from_str(&id.0).unwrap()
    )
    .fetch_one(&state.db_pool)
    .await
    .unwrap();

    let sim = SimulationRes {
        id: row.id.map_or(String::new(), |id| id.to_string()),
        team_id: row.team_id,
        chain_id: row.chain_id,
        block_at: row.block_at,
        transaction_version: row.transaction_version,
        nonce: row.nonce,
        max_fee: row.max_fee,
        cairo_version: row.cairo_version,
        wallet_address: row.wallet_address,
        calldata: row.calldata.map_or(Vec::new(), |calldata| calldata),
        created_at: row.created_at.assume_utc().unix_timestamp(),
        updated_at: row.updated_at.assume_utc().unix_timestamp(),
        status: row.status,
    };

    let rpc_client = create_rpc_client(sim.chain_id.clone());

    let tx_b = BroadcastedTransaction::Invoke(BroadcastedInvokeTransaction {
        sender_address: FieldElement::from_hex_be(sim.wallet_address.as_str()).unwrap(),
        calldata: convert_array(sim.calldata.clone())
            .iter()
            .map(|s| FieldElement::from_dec_str(s.as_str()).unwrap())
            .collect(),
        max_fee: FieldElement::from_dec_str("23200000090853470717981").unwrap(),
        signature: vec![],
        nonce: FieldElement::from_dec_str(sim.nonce.to_string().as_str()).unwrap(),
        is_query: false,
    });
    dbg!(tx_b.clone());
    let st = rpc_client
        .simulate_transaction(
            BlockId::Number(sim.block_at as u64),
            tx_b,
            [SimulationFlag::SkipValidate, SimulationFlag::SkipFeeCharge],
        )
        .await;

    match st {
        Ok(s) => Ok(Json(SimulateTraceResponse {
            simulated_transaction: s,
            simulation: sim,
        })),
        Err(_) => Err(StatusCode::NOT_FOUND),
    }
}
