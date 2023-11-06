To execute a transaction using forked node context follow

```
use blockifier::transaction::transaction_execution::Transaction;
use blockifier::transaction::account_transaction::AccountTransaction;
use blockifier::transaction::transactions::{InvokeTransaction};
use blockifier::state::cached_state::{CachedState, GlobalContractCache};
use blockifier::block_context::BlockContext;
use starknet_api::core::ChainId;
use starknet_api::block::BlockNumber;
use starknet_api::block::BlockTimestamp;
use blockifier::block_context::FeeTokenAddresses;
use blockifier::block_context::GasPrices;
use crate::forking::state::ForkStateReader;
use starknet::core::types::{BlockId};
use starknet_api::transaction::{TransactionHash, Transaction as StarknetApiTransaction, InvokeTransaction as SAInvokeTransaction, InvokeTransactionV1, L1HandlerTransaction};
use starknet_api::transaction::{Fee, TransactionSignature, TransactionVersion};
use starknet_api::hash::{StarkFelt, StarkHash};
use starknet_api::core::Nonce;
use starknet_api::core::{ContractAddress, PatriciaKey, EntryPointSelector};
use starknet_api::{calldata, contract_address, patricia_key, stark_felt};
use starknet_api::transaction::Calldata;
use starknet_crypto::FieldElement;
use blockifier::transaction::transactions::ExecutableTransaction;
use std::collections::HashMap;
use std::sync::Arc;
use blockifier::execution::execution_utils::sanitize_and_debug_call_info;


pub const OUTPUT_BUILTIN_NAME: &str = "output_builtin";
pub const HASH_BUILTIN_NAME: &str = "pedersen_builtin";
pub const RANGE_CHECK_BUILTIN_NAME: &str = "range_check_builtin";
pub const SIGNATURE_BUILTIN_NAME: &str = "ecdsa_builtin";
pub const BITWISE_BUILTIN_NAME: &str = "bitwise_builtin";
pub const EC_OP_BUILTIN_NAME: &str = "ec_op_builtin";
pub const KECCAK_BUILTIN_NAME: &str = "keccak_builtin";
pub const POSEIDON_BUILTIN_NAME: &str = "poseidon_builtin";
pub const SEGMENT_ARENA_BUILTIN_NAME: &str = "segment_arena_builtin";

const TEST_SEQUENCER_ADDRESS: &str = "0x1000";
const DEFAULT_ETH_L1_GAS_PRICE: u128 = 1 * u128::pow(10, 9); // Given in units of Wei.
const DEFAULT_STRK_L1_GAS_PRICE: u128 = 1 * u128::pow(10, 9); // Given in units of STRK.

pub fn create_fork_cached_state_at(
    block_id: BlockId,
    cache_dir: &str,
) -> CachedState<ForkStateReader> {
    CachedState::new(
        ForkStateReader::new("https://ofsg.mainnet-juno.rpc.nethermind.io", block_id, Some(cache_dir)),
        GlobalContractCache::default(),
    )
}

// pub fn create_cheatnet_state(state: &mut dyn State) -> BlockifierState {
//     let blockifier_state = BlockifierState::from(state);
//     // let cheatnet_state = CheatnetState::default();
//     // (blockifier_state, cheatnet_state)
//     blocifier_state
// }

fn default_resource_fee_costs() -> HashMap<String, f64> {
    const N_STEPS_FEE_WEIGHT: f64 = 0.01;

    HashMap::from([
        (
            blockifier::abi::constants::N_STEPS_RESOURCE.to_string(),
            N_STEPS_FEE_WEIGHT,
        ),
        (HASH_BUILTIN_NAME.to_string(), 32.0 * N_STEPS_FEE_WEIGHT),
        (
            RANGE_CHECK_BUILTIN_NAME.to_string(),
            16.0 * N_STEPS_FEE_WEIGHT,
        ),
        (
            SIGNATURE_BUILTIN_NAME.to_string(),
            2048.0 * N_STEPS_FEE_WEIGHT,
        ),
        (BITWISE_BUILTIN_NAME.to_string(), 64.0 * N_STEPS_FEE_WEIGHT),
        (POSEIDON_BUILTIN_NAME.to_string(), 32.0 * N_STEPS_FEE_WEIGHT),
        (OUTPUT_BUILTIN_NAME.to_string(), 0.0 * N_STEPS_FEE_WEIGHT),
        (EC_OP_BUILTIN_NAME.to_string(), 1024.0 * N_STEPS_FEE_WEIGHT),
        (KECCAK_BUILTIN_NAME.to_string(), 2048.0 * N_STEPS_FEE_WEIGHT),
    ])
}

fn build_block_context(block_number: u64) -> BlockContext {
    BlockContext {
        chain_id: ChainId("SN_MAIN".to_string()),
        block_number: BlockNumber(block_number),
        block_timestamp: BlockTimestamp::default(),
        sequencer_address: contract_address!(TEST_SEQUENCER_ADDRESS),
        fee_token_addresses: FeeTokenAddresses {
            eth_fee_token_address: contract_address!("0x049d36570d4e46f48e99674bd3fcc84644ddd6b96f7c741b1562b82f9e004dc7"),
            strk_fee_token_address: contract_address!("0x049d36570d4e46f48e99674bd3fcc84644ddd6b96f7c741b1562b82f9e004dc7"),
        },
        vm_resource_fee_cost: Arc::new(default_resource_fee_costs()),
        gas_prices: GasPrices {
            eth_l1_gas_price: DEFAULT_ETH_L1_GAS_PRICE,
            strk_l1_gas_price: DEFAULT_STRK_L1_GAS_PRICE,
        },
        invoke_tx_max_n_steps: 1_000_000,
        validate_max_n_steps: 1_000_000,
        max_recursion_depth: 50,
    }
}

fn tx_01() -> (Transaction, BlockContext, CachedState<ForkStateReader>) {
    let tx_raw = InvokeTransactionV1 {
        sender_address: ContractAddress::try_from(StarkFelt::from(FieldElement::from_hex_be("0x004d81c5284f13d7732380ab75a910aa363cf83bdee853472bdfcab1407fd77b").unwrap())).unwrap(),
        nonce: Nonce(StarkFelt::from(27_u8)),
        calldata: calldata![
            stark_felt!("0x2"),
            stark_felt!("0x49d36570d4e46f48e99674bd3fcc84644ddd6b96f7c741b1562b82f9e004dc7"),
            stark_felt!("0x219209e083275171774dab1df80982e9df2096516f06319c5c6d71ae0a8480c"),
            stark_felt!("0x3"),
            stark_felt!("0x10884171baf1914edc28d7afb619b40a4051cfae78a094a55d230f19e944a28"),
            stark_felt!("0x2386f26fc10000"),
            stark_felt!("0x0"),
            stark_felt!("0x10884171baf1914edc28d7afb619b40a4051cfae78a094a55d230f19e944a28"),
            stark_felt!("0x15543c3708653cda9d418b4ccd3be11368e40636c10c44b18cfe756b6d88b29"),
            stark_felt!("0x6"),
            stark_felt!("0x4"),
            stark_felt!("0x49d36570d4e46f48e99674bd3fcc84644ddd6b96f7c741b1562b82f9e004dc7"),
            stark_felt!("0x2386f26fc10000"),
            stark_felt!("0x0"),
            stark_felt!("0xe970d2"),
            stark_felt!("0x0")
        ],
        max_fee: Fee::default(),
        signature: TransactionSignature(
            vec![
                stark_felt!("0x30d291480cebbb2ded54ffed931f141f34d24b13e06ea39da9d60b60ab5aa67"),
                stark_felt!("0x9987e929f159f7eea5f900cd4aaef93af49a0a1531f6d5380557bcdd1e920d")
            ])
    };

    let tx_hash = TransactionHash(stark_felt!("0x054a018c0999916470ceb072a70ee78c2f9029ad34f113c27d16010dd78581a6"));

    let block_context = build_block_context(315778);
    let cached_fork_state = create_fork_cached_state_at(BlockId::Number(315778), "/tmp/sn-debugger/cache");

    (Transaction::from_api(StarknetApiTransaction::Invoke(SAInvokeTransaction::V1(tx_raw)), tx_hash, None, None, None).unwrap(), block_context, cached_fork_state)
}

fn tx_02() -> (Transaction, BlockContext, CachedState<ForkStateReader>) {
    let tx_raw = InvokeTransactionV1 {
        sender_address: contract_address!("0x02c0d6aa059b805bf07dcd1bbfae0e8531ca99322d92ee43ceae7973b4cc3f9c"),
        nonce: Nonce(StarkFelt::from(11_u8)),
        calldata: calldata![
            stark_felt!("0x2"),
            stark_felt!("0x53c91253bc9682c04929ca02ed00b3e423f6710d2ee7e0d5ebb06f3ecf368a8"),
            stark_felt!("0x219209e083275171774dab1df80982e9df2096516f06319c5c6d71ae0a8480c"),
            stark_felt!("0x3"),
            stark_felt!("0x7a0922657e550ba1ef76531454cb6d203d4d168153a0f05671492982c2f7741"),
            stark_felt!("0x50a327"),
            stark_felt!("0x0"),
            stark_felt!("0x7a0922657e550ba1ef76531454cb6d203d4d168153a0f05671492982c2f7741"),
            stark_felt!("0x2c0f7bf2d6cf5304c29171bf493feb222fef84bdaf17805a6574b0c2e8bcc87"),
            stark_felt!("0x9"),
            stark_felt!("0x50a327"),
            stark_felt!("0x0"),
            stark_felt!("0xb6b78370a76b8"),
            stark_felt!("0x0"),
            stark_felt!("0x2"),
            stark_felt!("0x53c91253bc9682c04929ca02ed00b3e423f6710d2ee7e0d5ebb06f3ecf368a8"),
            stark_felt!("0x49d36570d4e46f48e99674bd3fcc84644ddd6b96f7c741b1562b82f9e004dc7"),
            stark_felt!("0x2c0d6aa059b805bf07dcd1bbfae0e8531ca99322d92ee43ceae7973b4cc3f9c"),
            stark_felt!("0x652b4d0e")
        ],
        max_fee: Fee::default(),
        signature: TransactionSignature(
            vec![
                stark_felt!("0x69c5bcd0bcf37d7f9ad96738330da147f643c306c88a60b45185ab27788599e"),
                stark_felt!("0x27e5efb369e7771a4a15067d184bc35f0738d087cc1864f64271df2b052b645")
            ])
    };

    let tx_hash = TransactionHash(stark_felt!("0x02807cf2b5d7e1bde7cb81ff8e91a99de65d27dc3fb055da610356f541b2fbc7"));

    let block_context = build_block_context(287255);
    let cached_fork_state = create_fork_cached_state_at(BlockId::Number(287255), "/tmp/sn-debugger/cache");

    (Transaction::from_api(StarknetApiTransaction::Invoke(SAInvokeTransaction::V1(tx_raw)), tx_hash, None, None, None).unwrap(), block_context, cached_fork_state)
}

// Rejected Jediswap Transaction
fn tx_03() -> (Transaction, BlockContext, CachedState<ForkStateReader>) {
    let tx_raw = L1HandlerTransaction {
        version: TransactionVersion::ZERO,  // TODO: See if this version is correct
        nonce: Nonce(StarkFelt::from(1148139_u32)),
        contract_address: contract_address!("0x1b64371585074b2c333e8b9fea28198ed8b75efcec2f3e3f7650a63de2999c1"),
        entry_point_selector: EntryPointSelector(stark_felt!("0x240060cdb34fcc260f41eac7474ee1d7c80b7e3607daff9ac67c7ea2ebb1c44")),
        calldata: calldata![
            stark_felt!("0x50084c51f6d7e9801b6a7bdba85822db985465fe"),
            stark_felt!("0x1"),
            stark_felt!("0xda114221cb83fa859dbdb4c44beeaa0bb37c7537ad5ae66fe5e0efd20e6eb3"),
            stark_felt!("0x2b5e3af16b1880000"),
            stark_felt!("0x0"),
            stark_felt!("0x1"),
            stark_felt!("0x7e2a13b40fc1119ec55e0bcf9428eedaa581ab3c924561ad4e955f95da63138"),
            stark_felt!("0x63a311a5962d440"),
            stark_felt!("0x0"),
            stark_felt!("0x1"),
            stark_felt!("0xda114221cb83fa859dbdb4c44beeaa0bb37c7537ad5ae66fe5e0efd20e6eb3"),
            stark_felt!("0x29a303b928b9391ce797ec27d011d3937054bee783ca7831df792bae00c925c"),
            stark_felt!("0x4b74eb5f8cd2e8c8346072d939e3834a720b9e7f5157aaba7a36e47288b831"),
            stark_felt!("0xa"),
            stark_felt!("0x2"),
            stark_felt!("0xa"),
            stark_felt!("0xda114221cb83fa859dbdb4c44beeaa0bb37c7537ad5ae66fe5e0efd20e6eb3"),
            stark_felt!("0x7e2a13b40fc1119ec55e0bcf9428eedaa581ab3c924561ad4e955f95da63138"),
            stark_felt!("0x1"),
            stark_felt!("0x0"),
            stark_felt!("0x1"),
            stark_felt!("0x0"),
            stark_felt!("0x2"),
            stark_felt!("0xda114221cb83fa859dbdb4c44beeaa0bb37c7537ad5ae66fe5e0efd20e6eb3"),
            stark_felt!("0x49d36570d4e46f48e99674bd3fcc84644ddd6b96f7c741b1562b82f9e004dc7"),
            stark_felt!("0x0"),
            stark_felt!("0x780519fdc4e42079e328f9822f9a7f4f8a4692ea39e62447c8a176289c64ef3")
        ]
    };

    let tx_hash = TransactionHash(stark_felt!("0x7a34f3af88fc0def015a82b31d6081d7bda205dc69f8c659ebed02681fa93b7"));

    let block_context = build_block_context(208031); // TODO:
    let cached_fork_state = create_fork_cached_state_at(BlockId::Number(208031), "/tmp/sn-debugger/cache");

    let paid_fee_on_l1 = Fee(1_u128);
    (Transaction::from_api(StarknetApiTransaction::L1Handler(tx_raw), tx_hash, None, Some(paid_fee_on_l1), None).unwrap(), block_context, cached_fork_state)
}

fn main() {
    println!("Hello, world!");

    let (tx, block_context, mut cached_fork_state) = tx_03();

    let mut tx_state = CachedState::<_>::create_transactional(&mut cached_fork_state);
    let tx_info = tx.execute(&mut tx_state, &block_context, true, true);

    dbg!("----------------------------------");
    // sanitize_and_debug_call_info(tx_info.unwrap().execute_call_info.unwrap());
    // let mut blocifier_state = create_cheatnet_state(cached_fork_state);
    dbg!(tx_info.unwrap().execute_call_info.unwrap());
}

```
