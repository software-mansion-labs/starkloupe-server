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
use cheatnet::forking::state::ForkStateReader;
use cheatnet::runtime_extensions::call_to_blockifier_runtime_extension::execution::entry_point::execute_call_entry_point;
use cheatnet::runtime_extensions::call_to_blockifier_runtime_extension::rpc::CallResult;
use cheatnet::state::BlockInfoReader;
use cheatnet::state::CallTrace;
use cheatnet::state::CheatnetState;
use internal_tracing::InternalFnCallTraceEntryNode;
use serde::Deserialize;
use serde::Serialize;
use serde_json::Value;
use starknet::core::types::ContractClass;
use starknet::core::types::MaybePendingTransactionReceipt;
use starknet::core::types::TransactionReceipt;
use starknet::core::types::{FieldElement, InvokeTransaction, Transaction};
use starknet::providers::Provider;
use starknet_api::block::BlockNumber;
use starknet_api::core::ClassHash;
use starknet_api::core::EntryPointSelector;
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
use std::str::FromStr;
use std::sync::Arc;
use walnut_shared::create_rpc_client;

#[derive(Serialize, Deserialize, Debug)]
pub struct SimulationRawArgs {
    pub chain_id: String,
    pub block_number: u64,
    pub nonce: u64,
    pub sender_address: String,
    pub calldata: Vec<String>,
}

#[derive(Debug)]
pub struct SimulationArgs {
    pub chain_id: ChainId,
    pub block_number: BlockNumber,
    pub nonce: Nonce,
    pub sender_address: ContractAddress,
    pub calldata: Calldata,
}

impl From<SimulationRawArgs> for SimulationArgs {
    fn from(raw_args: SimulationRawArgs) -> Self {
        let calldata: Vec<StarkFelt> = raw_args
            .calldata
            .iter()
            .map(|x| stark_felt!(convert_to_hex(x).as_str()))
            .collect();
        Self {
            chain_id: ChainId(raw_args.chain_id.clone()),
            block_number: BlockNumber(raw_args.block_number),
            nonce: Nonce(StarkFelt::from(raw_args.nonce)),
            sender_address: contract_address!(raw_args.sender_address.as_str()),
            calldata: Calldata(calldata.into()),
        }
    }
}

#[derive(Serialize, Debug)]
pub struct SimulationInfo {
    pub call_trace: SimulationCallTrace,
}

pub fn simulate(args: SimulationArgs) -> SimulationInfo {
    let mut cached_fork_state =
        create_fork_cached_state_at(args.chain_id, args.block_number, "tmp/sn-debugger/cache");

    let entry_point_selector = selector_from_name(constants::EXECUTE_ENTRY_POINT_NAME);

    let mut execute_call = CallEntryPoint {
        entry_point_type: EntryPointType::External,
        entry_point_selector,
        calldata: args.calldata,
        class_hash: None,
        code_address: None,
        storage_address: args.sender_address,
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
                nonce: args.nonce,
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

    get_simulation_info(
        &cached_fork_state.state.fork_state_reader.unwrap(),
        cheatnet_state,
    )
}

#[derive(Serialize, Debug)]
pub struct SimulationCallTraceAdditionalInfo {
    entry_point_selector_name: Option<String>,
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
    pub additional_info: SimulationCallTraceAdditionalInfo,
}

fn get_simulation_info(
    fork_state_reader: &ForkStateReader,
    cheatnet_state: CheatnetState,
) -> SimulationInfo {
    SimulationInfo {
        call_trace: get_simulation_call_trace(
            fork_state_reader,
            cheatnet_state
                .trace_data
                .current_call_stack
                .borrow_full_trace(),
        ),
    }
}

fn get_simulation_call_trace(
    fork_state_reader: &ForkStateReader,
    call_trace_ref: Ref<CallTrace>,
) -> SimulationCallTrace {
    let mut nested_calls = Vec::new();
    for nested_call in &call_trace_ref.nested_calls {
        nested_calls.push(get_simulation_call_trace(
            fork_state_reader,
            nested_call.borrow(),
        ));
    }

    SimulationCallTrace {
        entry_point: call_trace_ref.entry_point.clone(),
        used_execution_resources: call_trace_ref.used_execution_resources.clone(),
        nested_calls,
        result: call_trace_ref.result.clone(),
        internal_fn_call_trace: call_trace_ref.internal_fn_call_trace.clone(),
        additional_info: get_additional_info(
            fork_state_reader,
            call_trace_ref.entry_point.class_hash,
            call_trace_ref.entry_point.entry_point_selector,
        ),
    }
}

#[derive(Serialize, Debug)]
pub struct TransactionSimulationResult {
    pub simulation_result: SimulationInfo,
    pub chain_id: String,
    pub block_number: u64,
    pub nonce: u64,
    pub sender_address: String,
    pub calldata: Vec<String>,
}

pub async fn simulate_transaction_by_hash(
    chain_id: ChainId,
    tx_hash: String,
) -> Option<TransactionSimulationResult> {
    let provider_client = create_rpc_client(&chain_id);
    let transaction_hash = FieldElement::from_str(&tx_hash.as_str()).unwrap();
    let transaction = provider_client
        .get_transaction_by_hash(transaction_hash)
        .await;
    if let Ok(transaction) = transaction {
        if let Some((nonce, sender_address, calldata)) = extract_submitted_tx(transaction) {
            let transaction_receipt = provider_client
                .get_transaction_receipt(transaction_hash)
                .await;
            if let Ok(transaction_receipt) = transaction_receipt {
                if let Some(block_number) = extract_transaction_receipt(transaction_receipt) {
                    let simulation_result = simulate(SimulationArgs {
                        chain_id: chain_id.clone(),
                        block_number,
                        nonce,
                        sender_address,
                        calldata: calldata.clone(),
                    });
                    let calldata = calldata
                        .0
                        .iter()
                        .map(|x| x.to_string())
                        .collect::<Vec<String>>();
                    return Some(TransactionSimulationResult {
                        simulation_result,
                        chain_id: chain_id.0,
                        block_number: block_number.0,
                        nonce: nonce.0.try_into().unwrap(),
                        sender_address: sender_address.0.to_string(),
                        calldata,
                    });
                }
            }
        }
    }
    None
}

fn extract_transaction_receipt(
    transaction_receipt: MaybePendingTransactionReceipt,
) -> Option<BlockNumber> {
    match transaction_receipt {
        MaybePendingTransactionReceipt::Receipt(receipt) => match receipt {
            TransactionReceipt::Invoke(invoke_receipt) => {
                Some(BlockNumber(invoke_receipt.block_number))
            }
            _ => None,
        },
        _ => None,
    }
}

fn extract_submitted_tx(transaction: Transaction) -> Option<(Nonce, ContractAddress, Calldata)> {
    match transaction {
        Transaction::Invoke(invoke_transaction) => match invoke_transaction {
            InvokeTransaction::V1(tx) => {
                let calldata: Vec<StarkFelt> =
                    tx.calldata.into_iter().map(|x| stark_felt!(x)).collect();
                Some((
                    Nonce(StarkFelt::from(tx.nonce)),
                    contract_address!(tx.sender_address),
                    Calldata(calldata.into()),
                ))
            }
            _ => None,
        },
        _ => None,
    }
}

fn get_additional_info(
    fork_state_reader: &ForkStateReader,
    class_hash: Option<ClassHash>,
    entry_point_selector: EntryPointSelector,
) -> SimulationCallTraceAdditionalInfo {
    let mut entry_point_selector_name: Option<String> = None;
    if let Some(class_hash) = class_hash {
        let contract_class = fork_state_reader.get_compiled_contract_class_from_cache(class_hash);
        if let Some(contract_class) = contract_class {
            match contract_class {
                ContractClass::Sierra(class) => {
                    entry_point_selector_name =
                        get_entry_point_selector_name(class.abi, entry_point_selector);
                }
                _ => {}
            };
        }
    }
    SimulationCallTraceAdditionalInfo {
        entry_point_selector_name,
    }
}

fn get_entry_point_selector_name(
    abi: String,
    entry_point_selector: EntryPointSelector,
) -> Option<String> {
    let abi_value: Value = serde_json::from_str(abi.as_str()).unwrap();
    let external_function_names = get_external_function_names(&abi_value);
    for external_function_name in external_function_names {
        let selector = selector_from_name(external_function_name.as_str());
        if selector == entry_point_selector {
            return Some(external_function_name);
        }
    }
    None
}

fn get_external_function_names(value: &Value) -> Vec<String> {
    let mut function_names = Vec::new();

    if let Value::Array(array) = value {
        for item in array {
            if let Value::Object(obj) = item {
                if obj.get("type") == Some(&Value::String("function".to_string()))
                    && obj.get("state_mutability") == Some(&Value::String("external".to_string()))
                {
                    if let Some(Value::String(name)) = obj.get("name") {
                        function_names.push(name.clone());
                    }
                } else if obj.get("type") == Some(&Value::String("interface".to_string())) {
                    if let Some(Value::Array(items)) = obj.get("items") {
                        function_names
                            .extend(get_external_function_names(&Value::Array(items.clone())));
                    }
                }
            }
        }
    }

    function_names
}
