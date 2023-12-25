use crate::app_state::AppState;
use crate::config::rpc_url;
use axum::{extract::State, http::StatusCode, Extension, Json};
use db;
use simulate::utils::{convert_to_hex, create_fork_cached_state_at, get_block_context};

use blockifier::state::cached_state::CachedState;
use blockifier::transaction::transaction_execution::Transaction;
use blockifier::transaction::transactions::ExecutableTransaction;

use serde::{Deserialize, Serialize};

use sqlx::types::Uuid;
use starknet::core::types::{BlockId, FieldElement};
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
    nonce: Option<u8>,
    max_fee: Option<u128>,
    cairo_version: String,
}

#[derive(Serialize)]
pub struct SimulateResult {}

pub async fn simulate(
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

    let calldata_raw: Vec<StarkFelt> = sim
        .calldata
        .unwrap_or_default()
        .iter()
        .map(|x| stark_felt!(convert_to_hex(x).as_str()))
        .collect();

    let tx_raw = InvokeTransactionV1 {
        sender_address: contract_address!(sim.wallet_address.as_str()),
        nonce: Nonce(StarkFelt::from(sim.nonce as u64)),
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

fn create_rpc_client(chain_id: String) -> JsonRpcClient<HttpTransport> {
    JsonRpcClient::new(HttpTransport::new(Url::parse(rpc_url(&chain_id)).unwrap()))
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
