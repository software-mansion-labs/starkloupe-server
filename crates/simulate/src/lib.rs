pub mod abi_processor;
pub mod contract_names;
pub mod utils;
use crate::utils::create_fork_cached_state_at;
use abi_processor::AbiProcessor;
use blockifier::abi::abi_utils::selector_from_name;
use blockifier::context::BlockContext;
use blockifier::context::ChainInfo;
use blockifier::context::TransactionContext;
use blockifier::execution::common_hints::ExecutionMode;
use blockifier::execution::entry_point::CallEntryPoint;
use blockifier::execution::entry_point::CallType;
use blockifier::execution::entry_point::EntryPointExecutionContext;
use blockifier::state::cached_state::CachedState;
use blockifier::transaction::constants;
use blockifier::transaction::objects::CommonAccountFields;
use blockifier::transaction::objects::CurrentTransactionInfo;
use blockifier::transaction::objects::TransactionInfo;
use blockifier::versioned_constants::VersionedConstants;
use cairo_felt::Felt252;
use cairo_vm::vm::runners::cairo_runner::ExecutionResources;
use cairo_vm::vm::trace::trace_entry::TraceEntry;
use calldata_decoder::decode_datas;
use cheatnet::forking::state::ForkStateReader;
use cheatnet::runtime_extensions::call_to_blockifier_runtime_extension::execution::entry_point::execute_call_entry_point;
use cheatnet::runtime_extensions::call_to_blockifier_runtime_extension::rpc::CallFailure;
use cheatnet::runtime_extensions::call_to_blockifier_runtime_extension::rpc::CallResult;
use cheatnet::runtime_extensions::forge_runtime_extension::cheatcodes::spy_events::Event;
use cheatnet::runtime_extensions::forge_runtime_extension::cheatcodes::spy_events::SpyTarget;
use cheatnet::state::BlockInfoReader;
use cheatnet::state::CallTrace;
use cheatnet::state::CheatnetState;
use cheatnet::state::ExtendedStateReader;
use contract_names::ContractNamesFetcher;
use internal_tracing::call_trace::InternalFnCallTraceEntryNode;
use internal_tracing::debugger_data_fetcher::fetch_classes_debugger_data;
use internal_tracing::debugger_data_maps_full_class_to_class;
use internal_tracing::get_internal_trace_and_debugger_data;
use internal_tracing::ClassDebuggerDataWithContractClass;
use internal_tracing::ContractCallDebuggerData;
use internal_tracing::SimulationDebuggerData;
use serde::Deserialize;
use serde::Serialize;
use serde_json::json;
use serde_json::Value;
use sqlx::Pool;
use sqlx::Postgres;
use starknet::core::types::ContractClass;
use starknet::core::types::ExecutionResult;
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
use starknet_selector_decoder::get_selector;
use std::cell::Ref;
use std::collections::BTreeMap;
use std::collections::HashMap;
use std::str::FromStr;
use std::sync::Arc;
use walnut_shared::clone_vm_trace;
use walnut_shared::extract_chain_id;
use walnut_shared::felt252_to_hex;
use walnut_shared::{
    create_rpc_client, decode_felt252, felt_vec_to_event_vec, EventItems, StructItems,
};

#[derive(Serialize, Deserialize, Debug)]
pub struct SimulationRawArgs {
    pub chain_id: String,
    pub block_number: u64,
    pub nonce: Option<u64>,
    pub sender_address: String,
    pub calldata: Vec<String>,
    pub transaction_version: usize,
}

#[derive(Debug)]
pub struct SimulationArgs {
    pub chain_id: ChainId,
    pub block_number: BlockNumber,
    pub nonce: Option<Nonce>,
    pub sender_address: ContractAddress,
    pub calldata: Calldata,
    pub transaction_version: TransactionVersion,
}

impl From<SimulationRawArgs> for SimulationArgs {
    fn from(raw_args: SimulationRawArgs) -> Self {
        let calldata: Vec<StarkFelt> = raw_args
            .calldata
            .iter()
            .map(|x| stark_felt!(x.as_str()))
            .collect();
        let chain_id = extract_chain_id(raw_args.chain_id.as_str());
        Self {
            chain_id,
            block_number: BlockNumber(raw_args.block_number),
            nonce: raw_args.nonce.map(|nonce| Nonce(StarkFelt::from(nonce))),
            sender_address: contract_address!(raw_args.sender_address.as_str()),
            calldata: Calldata(calldata.into()),
            transaction_version: match raw_args.transaction_version {
                0 => TransactionVersion::ZERO,
                1 => TransactionVersion::ONE,
                2 => TransactionVersion::TWO,
                3 => TransactionVersion::THREE,
                _ => {
                    panic!("Invalid transaction version");
                }
            },
        }
    }
}

#[derive(Serialize, Debug)]
pub struct SimulationInfo {
    pub call_trace: Option<SimulationCallTrace>,
    pub events_trace: Option<Vec<EventTrace>>,
    pub max_nested_error_level: usize,
    pub execution_result: Option<ExecutionResult>,
    pub simulation_debugger_data: Option<SimulationDebuggerData>,
}

pub async fn simulate(
    db_pool: &Pool<Postgres>,
    s3_client: &aws_sdk_s3::Client,
    args: SimulationArgs,
) -> SimulationInfo {
    let mut cached_fork_state = create_fork_cached_state_at(
        &args.chain_id,
        BlockNumber(args.block_number.clone().0 - 1),
        "tmp/sn-debugger/cache",
    );

    let chain_id = args.chain_id.clone();

    let cheatnet_state = run_simulation(args, &mut cached_fork_state);

    let (mut simulation_info, class_hashes) = get_simulation_info(
        &cached_fork_state.state.fork_state_reader.unwrap(),
        cheatnet_state,
    );

    let classes_debugger_data = fetch_classes_debugger_data(db_pool, s3_client, class_hashes).await;

    if let Some(mut simulation_call_trace) = simulation_info.call_trace.take() {
        enhance_call_trace_with_internal_trace_and_debugger_data(
            &mut simulation_call_trace,
            &classes_debugger_data,
        );

        simulation_info.simulation_debugger_data = Some(SimulationDebuggerData {
            classes_debugger_data: debugger_data_maps_full_class_to_class(classes_debugger_data),
        });
        simulation_info.call_trace = Some(simulation_call_trace);
    }

    if let (Some(mut call_trace), Some(mut event_trace)) = (
        simulation_info.call_trace.take(),
        simulation_info.events_trace.take(),
    ) {
        ContractNamesFetcher::new(&chain_id)
            .enhance_trace_with_contract_names(&mut call_trace, &mut event_trace)
            .await;
        simulation_info.call_trace = Some(call_trace);
        simulation_info.events_trace = Some(event_trace);
    }
    simulation_info
}

fn run_simulation(
    args: SimulationArgs,
    cached_fork_state: &mut CachedState<ExtendedStateReader>,
) -> CheatnetState {
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
                version: args.transaction_version,
                signature: TransactionSignature::default(),
                nonce: args.nonce.unwrap_or(Nonce::default()),
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
        spies: vec![SpyTarget::All],
        ..Default::default()
    };

    cheatnet_state.trace_data.is_vm_trace_needed = true;

    let _res = execute_call_entry_point(
        &mut execute_call,
        cached_fork_state,
        &mut cheatnet_state,
        &mut ExecutionResources::default(),
        &mut context,
    );

    let size = cheatnet_state.spy_events(SpyTarget::All);
    let (_, events) = cheatnet_state.fetch_events(&Felt252::from(size));
    let raw_events = felt_vec_to_event_vec(&events);
    cheatnet_state.detected_events = raw_events;

    cheatnet_state
}

#[derive(Serialize, Debug, Clone)]
pub struct EventAbi {
    event_name: String,
    event_arguments_names: Vec<String>,
    event_arguments_types: Vec<String>,
}

#[derive(Serialize, Debug)]
pub struct SimulationCallTraceAdditionalInfo {
    contract_name: Option<String>,
    entry_point_function_name: Option<String>,
    entry_point_interface_name: Option<String>,
    is_erc20_token: bool,
    erc20_token_name: Option<String>,
    erc20_token_symbol: Option<String>,
    error_message: Option<String>,
    function_result: Option<Value>,
    function_return_result_types: Option<Vec<String>>,
    function_arguments_names: Option<Vec<String>>,
    function_arguments_types: Option<Vec<String>>,
    calldata_decoded: Option<Value>,
    event_abi: Option<Vec<EventAbi>>,
    call_debugger_data: Option<ContractCallDebuggerData>,
    class_hash: Option<String>,
}

#[derive(Serialize, Debug)]
pub struct EventTrace {
    pub contract_name: String,
    pub event_name: String,
    pub event_arguments_names: Vec<String>,
    pub event_keys: Vec<String>,
    pub event_datas: Vec<String>,
}

#[derive(Serialize, Debug)]
pub struct SimulationCallTrace {
    pub entry_point: CallEntryPoint,
    pub used_execution_resources: ExecutionResources,
    // pub used_l1_resources: L1Resources,
    // pub used_syscalls: SyscallCounter,
    pub nested_calls: Vec<SimulationCallTrace>,
    pub nested_level: usize,
    pub result: CallResult,
    pub internal_fn_call_trace: Option<InternalFnCallTraceEntryNode>,
    pub additional_info: SimulationCallTraceAdditionalInfo,
    pub _vm_trace: Option<Vec<TraceEntry>>,
    pub _relocated_memory: Option<Vec<Option<Felt252>>>,
}

fn get_simulation_info(
    fork_state_reader: &ForkStateReader,
    cheatnet_state: CheatnetState,
) -> (SimulationInfo, Vec<String>) {
    let mut class_hashes: Vec<String> = Vec::new();
    let mut max_nested_error_level: usize = 0;

    let call_trace_ref: Ref<CallTrace> = cheatnet_state
        .trace_data
        .current_call_stack
        .borrow_full_trace();

    if call_trace_ref.nested_calls.is_empty() {
        return (
            SimulationInfo {
                call_trace: None,
                events_trace: None,
                max_nested_error_level,
                execution_result: None,
                simulation_debugger_data: Some(SimulationDebuggerData {
                    classes_debugger_data: HashMap::new(),
                }),
            },
            Vec::new(),
        );
    }

    let mut call_trace = get_simulation_call_trace(
        fork_state_reader,
        call_trace_ref.nested_calls[0].borrow(),
        0,
        &mut max_nested_error_level,
        &mut class_hashes,
    );

    let events_trace = get_event_trace(&cheatnet_state.detected_events, &call_trace);

    let mut execution_result: ExecutionResult = ExecutionResult::Succeeded;
    update_error_message(
        &mut call_trace,
        max_nested_error_level,
        &mut execution_result,
    );

    (
        SimulationInfo {
            call_trace: Some(call_trace),
            events_trace: Some(events_trace),
            max_nested_error_level,
            execution_result: Some(execution_result),
            simulation_debugger_data: None,
        },
        class_hashes,
    )
}

fn get_simulation_call_trace(
    fork_state_reader: &ForkStateReader,
    call_trace_ref: Ref<CallTrace>,
    nested_level: usize,
    max_nested_error_level: &mut usize,
    class_hashes: &mut Vec<String>,
) -> SimulationCallTrace {
    let mut nested_calls = Vec::new();
    if call_trace_ref.nested_calls.is_empty()
        && matches!(&call_trace_ref.result, CallResult::Failure(_))
    {
        *max_nested_error_level = nested_level;
    }

    for nested_call in &call_trace_ref.nested_calls {
        if let CallResult::Failure(_) = &call_trace_ref.result {
            if nested_level >= *max_nested_error_level {
                *max_nested_error_level = nested_level
            }
        };

        let nested_trace = get_simulation_call_trace(
            fork_state_reader,
            nested_call.borrow(),
            nested_level + 1,
            max_nested_error_level,
            class_hashes,
        );
        nested_calls.push(nested_trace);
    }

    if let Some(class_hash) = call_trace_ref.entry_point.class_hash {
        class_hashes.push(class_hash.to_string());
    }

    SimulationCallTrace {
        entry_point: call_trace_ref.entry_point.clone(),
        used_execution_resources: call_trace_ref.used_execution_resources.clone(),
        nested_calls,
        nested_level,
        result: call_trace_ref.result.clone(),
        internal_fn_call_trace: None,
        additional_info: get_additional_info(
            fork_state_reader,
            call_trace_ref.entry_point.class_hash,
            call_trace_ref.entry_point.entry_point_selector,
            call_trace_ref.result.clone(),
            call_trace_ref.entry_point.calldata.clone(),
        ),
        _vm_trace: call_trace_ref
            .vm_trace
            .as_ref()
            .map(|vm_trace| clone_vm_trace(vm_trace)),
        _relocated_memory: call_trace_ref.relocated_memory.clone(),
    }
}

fn get_event_trace(events: &Vec<Event>, call_trace: &SimulationCallTrace) -> Vec<EventTrace> {
    let mut events_trace: Vec<EventTrace> = Vec::new();
    for event in events {
        let contract_name = event.from.to_string();

        let keys_hex = felt252_to_hex(event.keys.to_vec()).unwrap();
        let mut event_name = keys_hex[0].to_string();
        let mut event_keys = Vec::new();
        if keys_hex.len() > 1 {
            event_keys = keys_hex[1..].to_vec();
        }
        let event_datas = felt252_to_hex(event.data.to_vec()).unwrap();
        let event_abi = find_call_trace(call_trace, &contract_name);
        let filtered_event_abi = event_abi.as_ref().and_then(|events| {
            events
                .iter()
                .find(|abi| {
                    let selector = selector_from_name(abi.event_name.as_str());
                    selector.0.to_string() == event_name
                })
                .cloned()
        });
        let mut event_arguments_names: Vec<String> = Vec::new();
        if filtered_event_abi.is_some() {
            event_name = filtered_event_abi.as_ref().unwrap().event_name.clone();
            event_arguments_names = filtered_event_abi
                .as_ref()
                .unwrap()
                .event_arguments_names
                .clone();
        }
        let event_trace = EventTrace {
            contract_name,
            event_name,
            event_arguments_names,
            event_keys,
            event_datas,
        };
        events_trace.push(event_trace);
    }

    events_trace
}

fn find_call_trace(call_trace: &SimulationCallTrace, contract_name: &str) -> Option<Vec<EventAbi>> {
    if call_trace.entry_point.storage_address.0.to_string() == contract_name {
        return call_trace.additional_info.event_abi.clone();
    }

    if !call_trace.nested_calls.is_empty() {
        for nested_call in &call_trace.nested_calls {
            if let Some(found_trace) = find_call_trace(nested_call, contract_name) {
                return Some(found_trace);
            }
        }
    }

    None
}

#[derive(Serialize, Debug)]
pub struct TransactionSimulationResult {
    pub simulation_result: SimulationInfo,
    pub chain_id: String,
    pub block_number: u64,
    pub nonce: Option<u64>,
    pub sender_address: String,
    pub calldata: Vec<String>,
    pub transaction_version: usize,
}

pub async fn simulate_by_data(
    db_pool: &Pool<Postgres>,
    s3_client: &aws_sdk_s3::Client,
    args: SimulationArgs,
) -> TransactionSimulationResult {
    let nonce: Option<u64> = match args.nonce {
        Some(nonce) => match nonce.0.try_into() {
            Ok(value) => Some(value),
            Err(_) => None,
        },
        None => None,
    };
    let chain_id = args.chain_id.clone().0.to_string();
    let block_number = args.block_number.0;
    let sender_address = args.sender_address.0.to_string();
    let calldata = args
        .calldata
        .0
        .iter()
        .map(|x| x.to_string())
        .collect::<Vec<String>>();

    let transaction_version: usize = args.transaction_version.0.try_into().unwrap();
    let simulation_result = simulate(db_pool, s3_client, args).await;

    TransactionSimulationResult {
        simulation_result,
        chain_id,
        block_number,
        nonce,
        sender_address,
        calldata,
        transaction_version,
    }
}

pub async fn simulate_transaction_by_hash(
    db_pool: &Pool<Postgres>,
    s3_client: &aws_sdk_s3::Client,
    chain_id: ChainId,
    tx_hash: String,
) -> Option<TransactionSimulationResult> {
    let provider_client = create_rpc_client(&chain_id);
    let transaction_hash = FieldElement::from_str(&tx_hash.as_str()).unwrap();
    let transaction = provider_client
        .get_transaction_by_hash(transaction_hash)
        .await;
    if let Ok(transaction) = transaction {
        if let Some((nonce, sender_address, calldata, transaction_version)) =
            extract_submitted_tx(transaction)
        {
            let transaction_receipt = provider_client
                .get_transaction_receipt(transaction_hash)
                .await;
            if let Ok(transaction_receipt) = transaction_receipt {
                if let Some(block_number) = extract_transaction_receipt(transaction_receipt) {
                    let simulation_result = simulate(
                        db_pool,
                        s3_client,
                        SimulationArgs {
                            chain_id: chain_id.clone(),
                            block_number,
                            nonce: Some(nonce),
                            sender_address,
                            calldata: calldata.clone(),
                            transaction_version,
                        },
                    )
                    .await;
                    let calldata = calldata
                        .0
                        .iter()
                        .map(|x| x.to_string())
                        .collect::<Vec<String>>();
                    let nonce = match nonce.0.try_into() {
                        Ok(value) => Some(value),
                        Err(_) => None,
                    };
                    return Some(TransactionSimulationResult {
                        simulation_result,
                        chain_id: chain_id.0,
                        block_number: block_number.0,
                        nonce,
                        sender_address: sender_address.0.to_string(),
                        calldata,
                        transaction_version: transaction_version.0.try_into().unwrap(),
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

fn extract_submitted_tx(
    transaction: Transaction,
) -> Option<(Nonce, ContractAddress, Calldata, TransactionVersion)> {
    match transaction {
        Transaction::Invoke(invoke_transaction) => match invoke_transaction {
            InvokeTransaction::V0(tx) => {
                let calldata: Vec<StarkFelt> =
                    tx.calldata.into_iter().map(|x| stark_felt!(x)).collect();
                Some((
                    Nonce::default(),
                    contract_address!(tx.contract_address),
                    Calldata(calldata.into()),
                    TransactionVersion::ONE,
                ))
            }
            InvokeTransaction::V1(tx) => {
                let calldata: Vec<StarkFelt> =
                    tx.calldata.into_iter().map(|x| stark_felt!(x)).collect();
                Some((
                    Nonce(StarkFelt::from(tx.nonce)),
                    contract_address!(tx.sender_address),
                    Calldata(calldata.into()),
                    TransactionVersion::ONE,
                ))
            }
            InvokeTransaction::V3(tx) => {
                let calldata: Vec<StarkFelt> =
                    tx.calldata.into_iter().map(|x| stark_felt!(x)).collect();
                Some((
                    Nonce(StarkFelt::from(tx.nonce)),
                    contract_address!(tx.sender_address),
                    Calldata(calldata.into()),
                    TransactionVersion::THREE,
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
    result: CallResult,
    calldata: Calldata,
) -> SimulationCallTraceAdditionalInfo {
    let mut additional_info = SimulationCallTraceAdditionalInfo {
        contract_name: None,
        entry_point_function_name: None,
        entry_point_interface_name: None,
        is_erc20_token: false,
        erc20_token_name: None,
        erc20_token_symbol: None,
        error_message: None,
        function_return_result_types: None,
        function_result: None,
        function_arguments_names: None,
        function_arguments_types: None,
        calldata_decoded: None,
        event_abi: None,
        call_debugger_data: None,
        class_hash: class_hash.map(|class_hash| class_hash.to_string()),
    };
    let mut struct_items: Vec<StructItems> = Vec::new();
    let mut event_items: Vec<EventItems> = Vec::new();
    if let Some(class_hash) = class_hash {
        let contract_class = fork_state_reader.get_compiled_contract_class_from_cache(class_hash);
        if let Some(contract_class) = contract_class {
            match contract_class {
                ContractClass::Sierra(class) => {
                    let mut abi_processor = AbiProcessor::new(entry_point_selector);
                    abi_processor.process_abi(class.abi);
                    additional_info.entry_point_function_name =
                        abi_processor.entry_point_function_name;
                    additional_info.entry_point_interface_name =
                        abi_processor.entry_point_interface_name;
                    additional_info.is_erc20_token = abi_processor.is_erc20_token;
                    additional_info.function_arguments_names =
                        abi_processor.function_arguments_names;
                    additional_info.function_arguments_types =
                        abi_processor.function_arguments_types;
                    additional_info.function_return_result_types =
                        abi_processor.function_return_result_types;
                    struct_items = abi_processor.struct_items;
                    event_items = abi_processor.event_items;
                }
                _ => {}
            };
        }
    }

    get_event_data(&mut additional_info, &event_items);
    get_function_name(&mut additional_info, &entry_point_selector);
    get_function_result(&mut additional_info, &result, &struct_items);
    get_function_arguments(&mut additional_info, &calldata, &struct_items);

    additional_info
}

//TODO better way to get event data do not include all in response - uneffecient
fn get_event_data(
    additional_info: &mut SimulationCallTraceAdditionalInfo,
    event_items: &Vec<EventItems>,
) {
    if !event_items.is_empty() {
        let mut event_abis: Vec<EventAbi> = Vec::new();
        for event_item in event_items {
            if !event_item.name.is_empty() {
                let event_name = event_item.name.to_string();
                let event_arguments_names: Vec<String> = event_item
                    .members
                    .iter()
                    .map(|x| x.names.to_string())
                    .collect();
                let event_arguments_types: Vec<String> = event_item
                    .members
                    .iter()
                    .map(|x| x.types.to_string())
                    .collect();
                let event_abi = EventAbi {
                    event_name,
                    event_arguments_names,
                    event_arguments_types,
                };
                event_abis.push(event_abi);
            }
        }
        additional_info.event_abi = Some(event_abis);
    }
}

fn get_function_name(
    additional_info: &mut SimulationCallTraceAdditionalInfo,
    entry_point_selector: &EntryPointSelector,
) {
    if additional_info.entry_point_function_name.is_none() {
        let entry_point_selector_str = entry_point_selector.0.to_string();
        let selector = get_selector(&entry_point_selector_str);
        match selector {
            Some(name) => additional_info.entry_point_function_name = Some(name.to_string()),
            None => additional_info.entry_point_function_name = None,
        }
    }
}

fn get_function_result(
    additional_info: &mut SimulationCallTraceAdditionalInfo,
    call_result: &CallResult,
    struct_items: &Vec<StructItems>,
) {
    if let CallResult::Success { ret_data } = call_result {
        if let Ok(ret_hex) = felt252_to_hex(ret_data.to_vec()) {
            if let Some(function_return_result_types) =
                additional_info.function_return_result_types.clone()
            {
                let decoded_result = decode_datas(
                    &ret_hex,
                    &function_return_result_types,
                    &vec![],
                    struct_items,
                    &mut 0,
                );
                additional_info.function_result = Some(json!(decoded_result));
            }
        } else {
            panic!("Failed to decode return data");
        }
    }
}

fn get_function_arguments(
    additional_info: &mut SimulationCallTraceAdditionalInfo,
    calldata: &Calldata,
    struct_items: &Vec<StructItems>,
) {
    if let (Some(function_arguments_types), Some(function_arguments_names)) = (
        additional_info.function_arguments_types.clone(),
        additional_info.function_arguments_names.clone(),
    ) {
        let calldata_hex: Vec<String> = calldata.0.iter().map(|x| x.to_string()).collect();
        let decoded_arguments = decode_datas(
            &calldata_hex,
            &function_arguments_types,
            &function_arguments_names,
            &struct_items,
            &mut 0,
        );

        additional_info.calldata_decoded = Some(json!(decoded_arguments));
    }
}

fn update_error_message(
    call_trace: &mut SimulationCallTrace,
    max_nested_error_level: usize,
    execution_result: &mut ExecutionResult,
) {
    if call_trace.nested_calls.is_empty() {
        return;
    }

    for nested_trace in &mut call_trace.nested_calls {
        if nested_trace.nested_level == max_nested_error_level {
            if let CallResult::Failure(failure) = &nested_trace.result {
                match failure {
                    CallFailure::Panic { panic_data } => {
                        match decode_felt252(panic_data.to_vec()) {
                            Ok(decoded) => {
                                nested_trace.additional_info.error_message = Some(decoded.clone());
                                *execution_result = ExecutionResult::Reverted { reason: decoded };
                            }
                            Err(_) => panic!("Failed to decode felt252"),
                        }
                    }
                    CallFailure::Error { msg } => {
                        nested_trace.additional_info.error_message = Some(msg.to_string());
                        *execution_result = ExecutionResult::Reverted {
                            reason: msg.to_string(),
                        };
                    }
                }
            }
        } else {
            update_error_message(nested_trace, max_nested_error_level, execution_result);
        }
    }
}

pub fn enhance_call_trace_with_internal_trace_and_debugger_data(
    simulation_call_trace: &mut SimulationCallTrace,
    classes_debugger_data: &HashMap<String, ClassDebuggerDataWithContractClass>,
) {
    let (internal_fn_call_trace, call_debugger_data) = match (
        simulation_call_trace.entry_point.class_hash,
        &simulation_call_trace._relocated_memory,
        &simulation_call_trace._vm_trace,
    ) {
        (Some(class_hash), Some(relocated_memory), Some(vm_trace)) => {
            match classes_debugger_data.get(&class_hash.to_string()) {
                Some(full_class_debugger_data) => {
                    match get_internal_trace_and_debugger_data(
                        relocated_memory,
                        vm_trace,
                        full_class_debugger_data,
                    ) {
                        Ok((internal_fn_call_trace, call_debugger_data)) => {
                            (Some(internal_fn_call_trace), Some(call_debugger_data))
                        }
                        Err(_) => {
                            println!("Failed to get internal fn call trace");
                            (None, None)
                        }
                    }
                }
                None => {
                    println!("Failed to get internal fn call trace");
                    (None, None)
                }
            }
        }
        _ => {
            println!("Not enough data to get internal fn call trace");
            (None, None)
        }
    };
    simulation_call_trace.internal_fn_call_trace = internal_fn_call_trace;
    simulation_call_trace.additional_info.call_debugger_data = call_debugger_data;
    simulation_call_trace._vm_trace = None;
    simulation_call_trace._relocated_memory = None;
    for nested_call in &mut simulation_call_trace.nested_calls {
        enhance_call_trace_with_internal_trace_and_debugger_data(
            nested_call,
            classes_debugger_data,
        );
    }
}
