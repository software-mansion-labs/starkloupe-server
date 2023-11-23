use crate::app_state::AppState;
use crate::db;
use axum::{extract::State, http::StatusCode, Extension, Json};
// use reqwest;
use serde::{Deserialize, Serialize};
// use serde_json;
use sqlx::types::Uuid;
use starknet::core::types::{
    BlockId, BroadcastedInvokeTransaction, BroadcastedTransaction, ExecuteInvocation, FieldElement,
    SimulationFlag, TransactionTrace,
};
use starknet_providers::{
    jsonrpc::{HttpTransport, JsonRpcClient},
    Provider,
};
use tracing::info;
// use std::collections::HashMap;
use std::sync::Arc;
use url::Url;

#[derive(Serialize, Deserialize)]
pub struct StarkNetTransaction {
    // Define your transaction structure here;
    chain_id: String,
    wallet_address: String,
    calldata: Vec<String>,
    nonce: u8,
    max_fee: Option<u128>,
    version: u8,
    cairo_version: Option<String>,
}

#[derive(Serialize)]
pub struct SimulateResult {}

pub async fn simulate(
    Extension(team): Extension<db::Team>,
    State(state): State<Arc<AppState>>,
    Json(payload): Json<StarkNetTransaction>,
) -> Result<Json<SimulateResult>, StatusCode> {
    let public_rpc_client = create_rpc_client(payload.chain_id.clone(), false);

    let block_number = public_rpc_client.block_number().await.unwrap();

    // TODO: Insert into database
    let mut sim = db::Simulation::default();
    sim.team_id = team.id;
    sim.chain_id = payload.chain_id;
    sim.block_at = block_number as i32;
    sim.transaction_version = payload.version as i32;
    sim.nonce = payload.nonce as i32;
    sim.max_fee = match payload.max_fee {
        Some(max_fee) => max_fee.to_string(),
        None => String::from(""),
    };
    sim.cairo_version = match payload.cairo_version {
        Some(version) => version,
        None => String::from(""),
    };
    sim.wallet_address = payload.wallet_address;
    sim.calldata = payload.calldata;

    // Insert into database
    let row: (Uuid,) = sqlx::query_as(
        "INSERT INTO simulations (team_id, chain_id, block_at, transaction_version, nonce, max_fee, cairo_version, wallet_address, calldata) VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9) RETURNING id")
        .bind(&sim.team_id)
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

    let tx_b = BroadcastedTransaction::Invoke(BroadcastedInvokeTransaction {
        sender_address: FieldElement::from_hex_be(sim.wallet_address.as_str()).unwrap(),
        calldata: convert_array(sim.calldata.clone())
            .iter()
            .map(|s| FieldElement::from_dec_str(s.as_str()).unwrap())
            .collect(),
        max_fee: FieldElement::from_dec_str("23440000000000000").unwrap(),
        signature: vec![],
        nonce: FieldElement::from_dec_str(sim.nonce.to_string().as_str()).unwrap(),
        is_query: false,
    });

    let st = public_rpc_client
        .simulate_transaction(
            BlockId::Number(sim.block_at as u64),
            tx_b,
            [SimulationFlag::SkipFeeCharge],
        )
        .await;

    if st.is_err() {
        info!("Simulation failed {}", st.err().unwrap());
        sqlx::query!(
            "UPDATE simulations SET status = $1 WHERE id = $2",
            "failure",
            id,
        )
        .execute(&state.db_pool)
        .await
        .unwrap();
    } else {
        match st.unwrap().transaction_trace {
            TransactionTrace::Invoke(t) => match t.execute_invocation {
                ExecuteInvocation::Reverted(r) => {
                    info!("Simulation reverted {}", r.revert_reason.as_str());
                    sqlx::query!(
                        "UPDATE simulations SET status = $1 WHERE id = $2",
                        "failure",
                        id,
                    )
                    .execute(&state.db_pool)
                    .await
                    .unwrap();
                }
                ExecuteInvocation::Success(_) => {
                    info!("Simulation succeeded");
                    sqlx::query!(
                        "UPDATE simulations SET status = $1 WHERE id = $2",
                        "success",
                        id,
                    )
                    .execute(&state.db_pool)
                    .await
                    .unwrap();
                }
            },
            _ => {
                info!("Simulation success");
                sqlx::query!(
                    "UPDATE simulations SET status = $1 WHERE id = $2",
                    "success",
                    id,
                )
                .execute(&state.db_pool)
                .await
                .unwrap();
            }
        };
    }

    // TODO(jainkunal): Execute the transaction in context of the block and get status

    // TODO(jainkunal): Update status to DB

    // TODO: Return

    Ok(Json(SimulateResult {}))
}

fn create_rpc_client(chain_id: String, is_private: bool) -> JsonRpcClient<HttpTransport> {
    let url = match is_private {
        true => match chain_id.as_str() {
            "0x534e5f474f45524c49" => "https://3dfa-54-87-10-131.ngrok-free.app",
            "0x534e5f4d41494e" => "https://0721-54-87-10-131.ngrok-free.app",
            _ => panic!("Invalid chain id"),
        },
        false => match chain_id.as_str() {
            "0x534e5f474f45524c49" => "https://ikah.goerli1-juno.rpc.nethermind.io",
            "0x534e5f4d41494e" => "https://ofsg.mainnet-juno.rpc.nethermind.io",
            _ => panic!("Invalid chain id"),
        },
    };
    JsonRpcClient::new(HttpTransport::new(Url::parse(url).unwrap()))
}

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

// async fn query_node(method: &str, params: HashMap<&str, &str>) -> serde_json::Value {
//     let json_payload = serde_json::json!({
//         "jsonrpc": "2.0",
//         "method": method,
//         "params": params,
//         "id": 1,
//     });

//     let node_url = "https://ofsg.mainnet-juno.rpc.nethermind.io";

//     let client = reqwest::Client::new();
//     let res = client
//         .post(node_url)
//         .json(&json_payload)
//         .send()
//         .await
//         .unwrap();

//     let data: HashMap<String, serde_json::Value> = res.json().await.unwrap();

//     data.get("result").unwrap().clone()
// }
