use blockifier::{
    blockifier::block::BlockInfo,
    context::{BlockContext, ChainInfo},
    // state::cached_state::{CachedState, GlobalContractCache, GLOBAL_CONTRACT_CACHE_SIZE_FOR_TEST},
    state::cached_state::CachedState,
    transaction::transaction_types::TransactionType,
    versioned_constants::VersionedConstants,
};
use cheatnet::{forking::state::ForkStateReader, state::ExtendedStateReader};
use num_bigint::BigUint;
use num_traits::Num;
use runtime::starknet::state::DictStateReader;
use starknet_api::block::BlockNumber;
use url::Url;

pub fn create_fork_cached_state_at(
    rpc_url: Url,
    block_number: BlockNumber,
    cache_dir: &str,
) -> CachedState<ExtendedStateReader> {
    CachedState::new(ExtendedStateReader {
        dict_state_reader: DictStateReader::default(),
        fork_state_reader: ForkStateReader::new(rpc_url, block_number, cache_dir).ok(),
    })
}

pub fn build_block_context(block_info: &BlockInfo) -> BlockContext {
    BlockContext::new_unchecked(
        block_info,
        &ChainInfo::default(),
        VersionedConstants::latest_constants(), // 0.13.1
    )
}

pub fn convert_to_hex(num_str: &str) -> String {
    let num = BigUint::from_str_radix(num_str, 10).unwrap();
    format!("{:x}", num)
}

//NOTE Move implementation here -> https://github.com/walnuthq/blockifier/blob/a6200402ab635d8a8e175f7f135be5914c960007/crates/blockifier/src/transaction/transaction_types.rs#L9
pub fn transaction_type_to_string(tx_type: TransactionType) -> String {
    match tx_type {
        TransactionType::Declare => "DECLARE".to_string(),
        TransactionType::DeployAccount => "DEPLOY".to_string(),
        TransactionType::InvokeFunction => "INVOKE".to_string(),
        TransactionType::L1Handler => "L1Handler".to_string(),
    }
}
