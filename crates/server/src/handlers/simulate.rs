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
    let rpc_client = create_rpc_client();

    let block_number = rpc_client.block_number().await.unwrap();

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

    dbg!(sim.clone());

    // Insert into database
    sqlx::query!(
        "INSERT INTO simulations (team_id, chain_id, block_at, transaction_version, nonce, max_fee, cairo_version, wallet_address, calldata) VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9)",
        sim.team_id,
        sim.chain_id,
        sim.block_at,
        sim.transaction_version,
        sim.nonce,
        sim.max_fee,
        sim.cairo_version,
        sim.wallet_address,
        &sim.calldata,
    ).execute(&state.db_pool).await.unwrap();

    // TODO(jainkunal): Execute the transaction in context of the block and get status

    // TODO(jainkunal): Update status to DB

    // TODO: Return

    Ok(Json(SimulateResult {}))
}

fn create_rpc_client() -> JsonRpcClient<HttpTransport> {
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
