use crate::app_state::AppState;
use crate::db;
use crate::utils::simulate::{convert_to_hex, create_fork_cached_state_at, get_block_context};
use axum::{extract::State, http::StatusCode, Extension, Json};

use blockifier::state::cached_state::CachedState;
use blockifier::transaction::transaction_execution::Transaction;
use blockifier::transaction::transactions::ExecutableTransaction;

use serde::{Deserialize, Serialize};

use sqlx::types::Uuid;
use starknet::core::types::BlockId;
use starknet_api::block::BlockNumber;
use starknet_api::core::{ChainId, ContractAddress, Nonce, PatriciaKey};
use starknet_api::hash::{StarkFelt, StarkHash};
use starknet_api::transaction::{
    Calldata, Fee, InvokeTransaction as SAInvokeTransaction, InvokeTransactionV1,
    Transaction as StarknetApiTransaction, TransactionHash, TransactionSignature,
};
use starknet_api::{contract_address, patricia_key, stark_felt};
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
    let private_rpc_client = create_rpc_client(payload.chain_id.clone(), true);

    let block_number = private_rpc_client.block_number().await.unwrap();

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

    let calldata_raw: Vec<StarkFelt> = sim
        .calldata
        .iter()
        .map(|x| stark_felt!(convert_to_hex(x).as_str()))
        .collect();

    let tx_raw = InvokeTransactionV1 {
        sender_address: contract_address!(sim.wallet_address.as_str()),
        nonce: Nonce(StarkFelt::from(payload.nonce)),
        calldata: Calldata(calldata_raw.into()),
        max_fee: Fee::default(),
        signature: TransactionSignature(vec![]),
    };

    let tx_hash = TransactionHash(StarkHash::default());
    let tx = Transaction::from_api(
        StarknetApiTransaction::Invoke(SAInvokeTransaction::V1(tx_raw)),
        tx_hash,
        None,
        None,
        None,
    )
    .unwrap();

    let chain_id = ChainId(sim.chain_id);
    let block_context = get_block_context(chain_id.clone(), BlockNumber(sim.block_at as u64));

    // TODO: Don't use File cache
    let mut cached_fork_state = create_fork_cached_state_at(
        chain_id,
        BlockId::Number(sim.block_at as u64),
        "/tmp/sn-debugger/cache",
    );

    let mut tx_state = CachedState::<_>::create_transactional(&mut cached_fork_state);

    let tx_info = tx.execute(&mut tx_state, &block_context, true, false);

    let sim_status = match tx_info {
        Ok(tx) => match tx.revert_error {
            Some(_) => "failure",
            None => "success",
        },
        Err(_) => "failure",
    };

    sqlx::query!(
        "UPDATE simulations SET status = $1 WHERE id = $2",
        sim_status,
        id,
    )
    .execute(&state.db_pool)
    .await
    .unwrap();

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
