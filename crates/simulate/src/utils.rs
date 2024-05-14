use blockifier::{
    blockifier::block::BlockInfo,
    context::{BlockContext, ChainInfo},
    // state::cached_state::{CachedState, GlobalContractCache, GLOBAL_CONTRACT_CACHE_SIZE_FOR_TEST},
    state::cached_state::CachedState,
    versioned_constants::VersionedConstants,
};
use cheatnet::{forking::state::ForkStateReader, state::ExtendedStateReader};
use num_bigint::BigUint;
use num_traits::Num;
use runtime::starknet::state::DictStateReader;
use starknet_api::{block::BlockNumber, core::ChainId};
use url::Url;

pub fn create_fork_cached_state_at(
    chain_id: &ChainId,
    block_number: BlockNumber,
    cache_dir: &str,
) -> CachedState<ExtendedStateReader> {
    let url = match chain_id.0.as_str() {
        "0x534e5f474f45524c49" => {
            "https://starknet-goerli.g.alchemy.com/v2/D2pgqj4yeZmmZyBY7tw-CMnO2nUL8n94"
        }
        "0x534e5f4d41494e" => {
            "https://starknet-mainnet.g.alchemy.com/v2/9J1ION8Owu9eHgZeyWlE9-N0yEepGA58"
        }
        _ => panic!("Invalid chain id"),
    };
    CachedState::new(
        ExtendedStateReader {
            dict_state_reader: DictStateReader::default(),
            fork_state_reader: ForkStateReader::new(
                Url::parse(url).unwrap(),
                block_number,
                cache_dir,
            )
            .ok(),
        },
        // GlobalContractCache::new(GLOBAL_CONTRACT_CACHE_SIZE_FOR_TEST),
    )
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
