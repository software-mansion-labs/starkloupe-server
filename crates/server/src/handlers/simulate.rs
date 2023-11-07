use crate::app_state::AppState;
use crate::db;
use axum::{extract::State, http::StatusCode, Extension, Json};
use reqwest;
use serde::{Deserialize, Serialize};
use serde_json;
use starknet_providers::{
    jsonrpc::{HttpTransport, JsonRpcClient},
    Provider,
};
use std::collections::HashMap;
use std::sync::Arc;
use url::Url;

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
    Extension(team): Extension<db::Team>,
    State(state): State<Arc<AppState>>,
    Json(payload): Json<StarkNetTransaction>,
) -> Result<Json<SimulateResult>, StatusCode> {
    let rpc_client = rpc_client();

    let block_number = rpc_client.block_number().await.unwrap();

    // TODO: Insert into database
    let mut sim = db::Simulation::default();
    sim.chain_id = payload.chain_id as i32;
    sim.block_at = block_number as i32;
    sim.transaction_version = payload.version as i32;
    sim.team_id = team.id;

    dbg!(sim.clone());

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

fn rpc_client() -> JsonRpcClient<HttpTransport> {
    JsonRpcClient::new(HttpTransport::new(
        Url::parse(
            std::env::var("NODE_URL")
                .unwrap_or("https://ofsg.mainnet-juno.rpc.nethermind.io".to_string())
                .as_str(),
        )
        .unwrap(),
    ))
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
