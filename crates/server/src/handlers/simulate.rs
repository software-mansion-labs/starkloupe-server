use crate::app_state::AppState;
use crate::db;
use axum::{extract::State, http::StatusCode, Json};
use reqwest;
use serde::{Deserialize, Serialize};
use serde_json;
use std::collections::HashMap;
use std::sync::Arc;

#[derive(Serialize, Deserialize)]
pub struct StarkNetTransaction {
    // Define your transaction structure here;
    chain_id: u32,
    contract_address: String,
    calldata: Vec<String>,
    nonce: u8,
    max_fee: u128,
    version: u8,
}

#[derive(Serialize)]
pub struct SimulateResult {}

pub async fn simulate(
    State(state): State<Arc<AppState>>,
    Json(payload): Json<StarkNetTransaction>,
) -> Result<Json<SimulateResult>, StatusCode> {
    let block_number = get_current_block_number().await;

    // TODO: Insert into database
    let mut sim = db::Simulation::default();
    sim.chain_id = payload.chain_id as i32;
    sim.block_at = block_number as i32;
    sim.transaction_version = payload.version as i32;

    // Insert into database
    sqlx::query!(
        "INSERT INTO simulations (team_id, chain_id, block_at, transaction_type, transaction_version) VALUES ($1, $2, $3, $4, $5)",
        sim.team_id,
        sim.chain_id,
        sim.block_at,
        sim.transaction_type,
        sim.transaction_version,
    ).execute(&state.db_pool).await.unwrap();

    // TODO(jainkunal): Execute the transaction in context of the block and get status

    // TODO(jainkunal): Update status to DB

    // TODO: Return

    Ok(Json(SimulateResult {}))
}

async fn get_current_block_number() -> u64 {
    let method = "starknet_blockNumber";
    let params = HashMap::new();

    let res_value = query_node(method, params).await;

    match res_value {
        serde_json::Value::Number(n) => n.as_u64().unwrap_or(0),
        _ => 0,
    }
}

async fn query_node(method: &str, params: HashMap<&str, &str>) -> serde_json::Value {
    let json_payload = serde_json::json!({
        "jsonrpc": "2.0",
        "method": method,
        "params": params,
        "id": 1,
    });

    let node_url = "https://ofsg.mainnet-juno.rpc.nethermind.io";

    let client = reqwest::Client::new();
    let res = client
        .post(node_url)
        .json(&json_payload)
        .send()
        .await
        .unwrap();

    let data: HashMap<String, serde_json::Value> = res.json().await.unwrap();

    data.get("result").unwrap().clone()
}
