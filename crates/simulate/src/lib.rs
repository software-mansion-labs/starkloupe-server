pub mod utils;

use crate::utils::convert_to_hex;
use crate::utils::create_fork_cached_state_at;
use blockifier::abi::abi_utils::selector_from_name;
use blockifier::context::BlockContext;
use blockifier::context::ChainInfo;
use blockifier::context::TransactionContext;
use blockifier::execution::common_hints::ExecutionMode;
use blockifier::execution::entry_point::CallEntryPoint;
use blockifier::execution::entry_point::CallType;
use blockifier::execution::entry_point::EntryPointExecutionContext;
use blockifier::transaction::constants;
use blockifier::transaction::objects::CommonAccountFields;
use blockifier::transaction::objects::CurrentTransactionInfo;
use blockifier::transaction::objects::TransactionInfo;
use blockifier::versioned_constants::VersionedConstants;
use cairo_vm::vm::runners::cairo_runner::ExecutionResources;
use cheatnet::runtime_extensions::call_to_blockifier_runtime_extension::execution::entry_point::execute_call_entry_point;
use cheatnet::runtime_extensions::call_to_blockifier_runtime_extension::rpc::CallResult;
use cheatnet::state::BlockInfoReader;
use cheatnet::state::CallTrace;
use cheatnet::state::CheatnetState;
use internal_tracing::InternalFnCallTraceEntryNode;
use serde::Deserialize;
use serde::Serialize;
use starknet_api::block::BlockNumber;
use starknet_api::core::{ChainId, ContractAddress, Nonce, PatriciaKey};
use starknet_api::data_availability::DataAvailabilityMode;
use starknet_api::deprecated_contract_class::EntryPointType;
use starknet_api::hash::{StarkFelt, StarkHash};
use starknet_api::transaction::Resource;
use starknet_api::transaction::ResourceBounds;
use starknet_api::transaction::ResourceBoundsMapping;
use starknet_api::transaction::TransactionVersion;
use starknet_api::transaction::{Calldata, TransactionHash, TransactionSignature};
use starknet_api::{contract_address, patricia_key, stark_felt};
use std::cell::Ref;
use std::collections::BTreeMap;
use std::sync::Arc;

#[derive(Serialize, Deserialize, Debug)]
pub struct SimulationArgs {
    pub chain_id: String,
    pub block_at: u64,
    pub nonce: u64,
    pub wallet_address: String,
    pub calldata: Vec<String>,
}

#[derive(Serialize, Debug)]
pub struct SimulationInfo {
    pub call_trace: SimulationCallTrace,
}

pub fn simulate(sim: SimulationArgs) -> SimulationInfo {
    let calldata_raw: Vec<StarkFelt> = sim
        .calldata
        .iter()
        .map(|x| stark_felt!(convert_to_hex(x).as_str()))
        .collect();

    let chain_id = ChainId(sim.chain_id.clone());

    let mut cached_fork_state = create_fork_cached_state_at(
        chain_id,
        BlockNumber(sim.block_at),
        "/tmp/sn-debugger/cache",
    );

    let entry_point_selector = selector_from_name(constants::EXECUTE_ENTRY_POINT_NAME);
    let storage_address = contract_address!(sim.wallet_address.as_str());

    let mut execute_call = CallEntryPoint {
        entry_point_type: EntryPointType::External,
        entry_point_selector,
        calldata: Calldata(calldata_raw.into()),
        class_hash: None,
        code_address: None,
        storage_address,
        caller_address: ContractAddress::default(),
        call_type: CallType::Call,
        initial_gas: u64::MAX,
    };

    let block_info = cached_fork_state.state.get_block_info().unwrap();

    let transaction_context = Arc::new(TransactionContext {
        block_context: BlockContext::new_unchecked(
            &block_info,
            &ChainInfo::default(),
            VersionedConstants::latest_constants(),
        ),
        tx_info: TransactionInfo::Current(CurrentTransactionInfo {
            common_fields: CommonAccountFields {
                transaction_hash: TransactionHash::default(),
                version: TransactionVersion::ONE,
                signature: TransactionSignature::default(),
                nonce: Nonce(StarkFelt::from(sim.block_at)),
                sender_address: ContractAddress::default(),
                only_query: false,
            },
            resource_bounds: ResourceBoundsMapping(BTreeMap::from([
                (
                    Resource::L1Gas,
                    ResourceBounds {
                        max_amount: 0,
                        max_price_per_unit: 1,
                    },
                ),
                (
                    Resource::L2Gas,
                    ResourceBounds {
                        max_amount: 0,
                        max_price_per_unit: 0,
                    },
                ),
            ])),
            tip: Default::default(),
            nonce_data_availability_mode: DataAvailabilityMode::L1,
            fee_data_availability_mode: DataAvailabilityMode::L1,
            paymaster_data: Default::default(),
            account_deployment_data: Default::default(),
        }),
    });

    let mut context =
        EntryPointExecutionContext::new(transaction_context, ExecutionMode::Execute, false)
            .unwrap();

    let mut cheatnet_state = CheatnetState {
        block_info,
        ..Default::default()
    };

    cheatnet_state.trace_data.is_vm_trace_needed = true;

    let res = execute_call_entry_point(
        &mut execute_call,
        &mut cached_fork_state,
        &mut cheatnet_state,
        &mut ExecutionResources::default(),
        &mut context,
    );

    dbg!(&cheatnet_state.trace_data);

    get_simulation_info(cheatnet_state)
}

#[derive(Serialize, Debug)]
pub struct SimulationCallTrace {
    pub entry_point: CallEntryPoint,
    pub used_execution_resources: ExecutionResources,
    // pub used_l1_resources: L1Resources,
    // pub used_syscalls: SyscallCounter,
    pub nested_calls: Vec<SimulationCallTrace>,
    pub result: CallResult,
    pub internal_fn_call_trace: Option<InternalFnCallTraceEntryNode>,
}

fn get_simulation_info(cheatnet_state: CheatnetState) -> SimulationInfo {
    SimulationInfo {
        call_trace: get_simulation_call_trace(
            cheatnet_state
                .trace_data
                .current_call_stack
                .borrow_full_trace(),
        ),
    }
}

fn get_simulation_call_trace(call_trace_ref: Ref<CallTrace>) -> SimulationCallTrace {
    let mut nested_calls = Vec::new();
    for nested_call in &call_trace_ref.nested_calls {
        nested_calls.push(get_simulation_call_trace(nested_call.borrow()));
    }

    SimulationCallTrace {
        entry_point: call_trace_ref.entry_point.clone(),
        used_execution_resources: call_trace_ref.used_execution_resources.clone(),
        nested_calls,
        result: call_trace_ref.result.clone(),
        internal_fn_call_trace: call_trace_ref.internal_fn_call_trace.clone(),
    }
}
