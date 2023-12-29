pub mod utils;

use crate::utils::convert_to_hex;
use crate::utils::create_fork_cached_state_at;
use crate::utils::get_block_context;
use blockifier::state::cached_state::CachedState;
use blockifier::transaction::errors::TransactionExecutionError;
use blockifier::transaction::objects::TransactionExecutionInfo;
use blockifier::transaction::transaction_execution::Transaction;
use blockifier::transaction::transactions::ExecutableTransaction;
use serde::Deserialize;
use serde::Serialize;
use starknet::core::types::BlockId;
use starknet::core::types::{
    CallType, EventContent, ExecuteInvocation, FeeEstimate, FieldElement, FunctionInvocation,
    InvokeTransactionTrace, SimulatedTransaction, TransactionTrace,
};
use starknet_api::block::BlockNumber;
use starknet_api::core::{ChainId, ContractAddress, Nonce, PatriciaKey};
use starknet_api::hash::{StarkFelt, StarkHash};
use starknet_api::transaction::{
    Calldata, Fee, InvokeTransaction as SAInvokeTransaction, InvokeTransactionV1,
    Transaction as StarknetApiTransaction, TransactionHash, TransactionSignature,
};
use starknet_api::{contract_address, patricia_key, stark_felt};

#[derive(Serialize, Deserialize, Debug)]
pub struct SimulationArgs {
    pub chain_id: String,
    pub block_at: u64,
    pub nonce: u64,
    pub wallet_address: String,
    pub calldata: Vec<String>,
}

pub fn simulate(
    sim: SimulationArgs,
) -> Result<TransactionExecutionInfo, TransactionExecutionError> {
    let calldata_raw: Vec<StarkFelt> = sim
        .calldata
        .iter()
        .map(|x| stark_felt!(convert_to_hex(x).as_str()))
        .collect();

    let tx_raw = InvokeTransactionV1 {
        sender_address: contract_address!(sim.wallet_address.as_str()),
        nonce: Nonce(StarkFelt::from(sim.nonce)),
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
    let block_context = get_block_context(chain_id.clone(), BlockNumber(sim.block_at));

    let mut cached_fork_state = create_fork_cached_state_at(
        chain_id,
        BlockId::Number(sim.block_at),
        "/tmp/sn-debugger/cache",
    );

    let mut tx_state = CachedState::<_>::create_transactional(&mut cached_fork_state);

    let tx_info = tx.execute(&mut tx_state, &block_context, true, false);

    tx_info
}

pub fn to_simulated_transaction(tx: TransactionExecutionInfo) -> SimulatedTransaction {
    let eci = tx.execute_call_info.unwrap();

    SimulatedTransaction {
        transaction_trace: TransactionTrace::Invoke(InvokeTransactionTrace {
            execute_invocation: ExecuteInvocation::Success(to_function_invocation(&eci)),
            // TODO: Implement this
            validate_invocation: None,
            fee_transfer_invocation: None,
        }),
        // TODO: Implement this
        fee_estimation: FeeEstimate {
            gas_consumed: 0,
            gas_price: 0,
            overall_fee: 0,
        },
    }
}

fn to_function_invocation(eci: &blockifier::execution::call_info::CallInfo) -> FunctionInvocation {
    let calldata = eci
        .call
        .calldata
        .0
        .iter()
        .map(|x| FieldElement::from_byte_slice_be(x.bytes()).unwrap())
        .collect();

    let events: Vec<EventContent> = eci
        .execution
        .events
        .iter()
        .map(|x| EventContent {
            data: x
                .event
                .data
                .0
                .iter()
                .map(|x| FieldElement::from(*x))
                .collect(),
            keys: x
                .event
                .keys
                .iter()
                .map(|x| FieldElement::from(x.0))
                .collect(),
        })
        .collect();

    let result = eci
        .execution
        .retdata
        .0
        .iter()
        .map(|x| FieldElement::from(*x))
        .collect();

    let internal_calls: Vec<FunctionInvocation> = eci
        .inner_calls
        .iter()
        .map(|x| to_function_invocation(x))
        .collect();

    FunctionInvocation {
        calldata: calldata,
        contract_address: FieldElement::from(*Into::<&StarkFelt>::into(
            eci.call.storage_address.0.key(),
        )),
        call_type: match eci.call.call_type {
            blockifier::execution::entry_point::CallType::Call => CallType::Call,
            blockifier::execution::entry_point::CallType::Delegate => CallType::LibraryCall,
        },
        caller_address: FieldElement::from(*Into::<&StarkFelt>::into(
            eci.call.caller_address.0.key(),
        )),
        class_hash: FieldElement::from(eci.call.class_hash.unwrap().0),
        entry_point_type: match eci.call.entry_point_type {
            starknet_api::deprecated_contract_class::EntryPointType::Constructor => {
                starknet::core::types::EntryPointType::Constructor
            }
            starknet_api::deprecated_contract_class::EntryPointType::External => {
                starknet::core::types::EntryPointType::External
            }
            starknet_api::deprecated_contract_class::EntryPointType::L1Handler => {
                starknet::core::types::EntryPointType::L1Handler
            }
        },
        events: events,
        // TODO: Implement this
        messages: vec![],
        result: result,
        entry_point_selector: FieldElement::from(eci.call.entry_point_selector.0),
        calls: internal_calls,
    }
}
