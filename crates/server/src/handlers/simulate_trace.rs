use crate::app_state::AppState;
use crate::handlers::simulations::SimulationRes;
use crate::types::TransactionExecutionInfo;
use crate::utils::simulate::convert_to_hex;
use crate::utils::simulate::create_fork_cached_state_at;
use crate::utils::simulate::get_block_context;
use axum::{
    extract::{Path, State},
    http::StatusCode,
    Json,
};
use blockifier::state::cached_state::CachedState;
use blockifier::transaction::transaction_execution::Transaction;
use blockifier::transaction::transactions::ExecutableTransaction;
use serde::Serialize;
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

use std::{str::FromStr, sync::Arc};

#[derive(Serialize)]
pub struct SimulateTraceResponse {
    execution_info: TransactionExecutionInfo,
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

    let calldata_raw: Vec<StarkFelt> = sim
        .calldata
        .clone()
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

    let chain_id = ChainId(sim.chain_id.clone());
    let block_context = get_block_context(chain_id.clone(), BlockNumber(sim.block_at as u64));

    // TODO: Don't use File cache
    let mut cached_fork_state = create_fork_cached_state_at(
        chain_id,
        BlockId::Number(sim.block_at as u64),
        "/tmp/sn-debugger/cache",
    );

    let mut tx_state = CachedState::<_>::create_transactional(&mut cached_fork_state);

    let tx_info = tx.execute(&mut tx_state, &block_context, true, false);
    // dbg!(tx_info);

    match tx_info {
        Ok(tx_info) => Ok(Json(SimulateTraceResponse {
            execution_info: tx_info.into(),
            simulation: sim,
        })),
        Err(_) => Err(StatusCode::EXPECTATION_FAILED),
    }
}
