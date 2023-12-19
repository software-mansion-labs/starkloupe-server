pub mod utils;

use crate::utils::convert_to_hex;
use crate::utils::create_fork_cached_state_at;
use crate::utils::get_block_context;
use blockifier::state::cached_state::CachedState;
use blockifier::transaction::errors::TransactionExecutionError;
use blockifier::transaction::objects::TransactionExecutionInfo;
use blockifier::transaction::transaction_execution::Transaction;
use blockifier::transaction::transactions::ExecutableTransaction;
use serde::Serialize;
use starknet::core::types::BlockId;
use starknet_api::block::BlockNumber;
use starknet_api::core::{ChainId, ContractAddress, Nonce, PatriciaKey};
use starknet_api::hash::{StarkFelt, StarkHash};
use starknet_api::transaction::{
    Calldata, Fee, InvokeTransaction as SAInvokeTransaction, InvokeTransactionV1,
    Transaction as StarknetApiTransaction, TransactionHash, TransactionSignature,
};
use starknet_api::{contract_address, patricia_key, stark_felt};

#[derive(Serialize)]
pub struct SimulationRes {
    pub id: String,
    pub project_id: i32,
    pub chain_id: String,
    pub block_at: i32,
    pub transaction_version: i32,
    pub nonce: i32,
    pub max_fee: String,
    pub cairo_version: String,
    pub wallet_address: String,
    pub calldata: Vec<String>,
    pub created_at: i64,
    pub updated_at: i64,
    pub status: String,
}

pub fn simulate(
    sim: &SimulationRes,
) -> Result<TransactionExecutionInfo, TransactionExecutionError> {
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

    let mut cached_fork_state = create_fork_cached_state_at(
        chain_id,
        BlockId::Number(sim.block_at as u64),
        "/tmp/sn-debugger/cache",
    );

    let mut tx_state = CachedState::<_>::create_transactional(&mut cached_fork_state);

    let tx_info = tx.execute(&mut tx_state, &block_context, true, false);

    tx_info
}
