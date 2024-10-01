pub mod abi_processor;
pub mod contract_names;
pub mod state;
pub mod utils;

use abi_processor::AbiProcessor;
use blockifier::abi::abi_utils::selector_from_name;
use blockifier::bouncer::BouncerConfig;
use blockifier::context::BlockContext;
use blockifier::context::ChainInfo;
use blockifier::context::FeeTokenAddresses;
use blockifier::context::TransactionContext;
use blockifier::execution::common_hints::ExecutionMode;
use blockifier::execution::entry_point::CallEntryPoint;
use blockifier::execution::entry_point::CallType;
use blockifier::execution::entry_point::EntryPointExecutionContext;
use blockifier::state::cached_state::CachedState;
use blockifier::state::errors::StateError;
use blockifier::transaction::constants;
use blockifier::transaction::errors::TransactionExecutionError;
use blockifier::transaction::objects::CommonAccountFields;
use blockifier::transaction::objects::CurrentTransactionInfo;
use blockifier::transaction::objects::TransactionInfo;
use blockifier::transaction::transaction_types::TransactionType;
use blockifier::versioned_constants::VersionedConstants;
use cairo_vm::vm::runners::cairo_runner::ExecutionResources;
use cairo_vm::vm::trace::trace_entry::RelocatedTraceEntry;
use cheatnet::runtime_extensions::call_to_blockifier_runtime_extension::execution::entry_point::execute_call_entry_point;
use cheatnet::runtime_extensions::call_to_blockifier_runtime_extension::rpc::CallFailure;
use cheatnet::runtime_extensions::call_to_blockifier_runtime_extension::rpc::CallResult;
use cheatnet::runtime_extensions::forge_runtime_extension::cheatcodes::spy_events::Event;
use cheatnet::state::BlockInfoReader;
use cheatnet::state::CallTrace;
use cheatnet::state::CallTraceNode;
use cheatnet::state::CheatnetState;
use contract_names::ContractNamesFetcher;
use data_decoder::calldata_decoder::decode_datas;
use internal_tracing::call_trace::InternalFnCallTraceEntryNode;
use internal_tracing::debugger_data_fetcher::fetch_classes_debugger_data;
use internal_tracing::debugger_data_maps_full_class_to_class;
use internal_tracing::get_internal_trace_and_debugger_data;
use internal_tracing::ClassDebuggerDataWithContractClass;
use internal_tracing::ContractCallDebuggerData;
use internal_tracing::SimulationDebuggerData;
use num_traits::ToPrimitive;
use serde::Deserialize;
use serde::Serialize;
use serde_json::json;
use serde_json::Value;
use sqlx::Pool;
use sqlx::Postgres;
use starknet::core::types::ContractClass;
use starknet::core::types::ExecutionResult;
use starknet::core::types::Felt;
use starknet_api::block::BlockNumber;
use starknet_api::block::BlockTimestamp;
use starknet_api::core::ClassHash;
use starknet_api::core::EntryPointSelector;
use starknet_api::core::{ChainId, ContractAddress, Nonce, PatriciaKey};
use starknet_api::data_availability::DataAvailabilityMode;
use starknet_api::deprecated_contract_class::EntryPointType;
use starknet_api::transaction::Resource;
use starknet_api::transaction::ResourceBounds;
use starknet_api::transaction::ResourceBoundsMapping;
use starknet_api::transaction::TransactionVersion;
use starknet_api::transaction::{Calldata, TransactionHash, TransactionSignature};
use starknet_api::{contract_address, felt, patricia_key};
use starknet_old::core::types as starknet_old_types;
use starknet_providers::jsonrpc::HttpTransport;
use starknet_providers::JsonRpcClient;
use starknet_providers::Provider;
use starknet_providers::ProviderError;
use starknet_selector_decoder::get_selector;
use state::ForkStateReader;
use std::cell::Ref;
use std::collections::BTreeMap;
use std::collections::HashMap;
use std::str::FromStr;
use std::sync::Arc;
use std::usize;
use thiserror::Error;
use tracing::error;
use url::Url;
use utils::transaction_type_to_string;
use walnut_shared::decode_felt;
use walnut_shared::felt_to_field_element;
use walnut_shared::felt_vec_to_hex_vec;
use walnut_shared::field_element_to_felt;
use walnut_shared::vec_field_element_to_vec_felt;
use walnut_shared::EnumItems;
use walnut_shared::{
    chain_id_to_readable_string, clone_vm_trace, create_rpc_client_from_url, extract_chain_id,
    get_contract_call_id, rpc_url, EventItems, StructItems, ETH_FEE_TOKEN_ADDRESS,
    STRK_FEE_TOKEN_ADDRESS,
};

#[derive(Serialize, Deserialize, Debug)]
pub struct SimulationRawArgs {
    pub chain_id: Option<String>,
    pub rpc_url: Option<String>,
    pub block_number: Option<u64>,
    pub nonce: Option<u64>,
    pub sender_address: String,
    pub calldata: Vec<String>,
    pub transaction_version: usize,
    pub transaction_signature: Option<Vec<Felt>>,
}

#[derive(Debug)]
pub struct SimulationArgs {
    pub chain_id: Option<ChainId>,
    pub rpc_url: Url,
    pub block_number: BlockNumber,
    pub nonce: Option<Nonce>,
    pub sender_address: ContractAddress,
    pub calldata: Vec<Felt>,
    pub transaction_version: TransactionVersion,
    pub transaction_signature: Option<TransactionSignature>,
}

#[derive(Serialize, Debug)]
pub struct TransactionSimulationResult {
    pub simulation_result: SimulationInfo,
    pub chain_id: Option<String>,
    pub block_number: u64,
    pub block_timestamp: u64,
    pub nonce: Option<u64>,
    pub sender_address: String,
    pub calldata: Vec<String>,
    pub transaction_version: usize,
    pub transaction_type: String,
    pub transaction_index_in_block: usize,
}

#[derive(Error, Debug)]
pub enum TransactionSimulationError {
    #[error("{0}")]
    StateError(#[from] StateError),
    #[error("{0}")]
    ProviderError(#[from] ProviderError),
    #[error("{0}")]
    PendingBlock(String),
    #[error("{0}")]
    TransactionExecutionError(#[from] TransactionExecutionError),
    #[error("Transaction hash not found")]
    TransactionHashNotFound,
    #[error("Invalid chain id")]
    InvalidChainId,
    #[error("Invalid RPC URL")]
    InvalidRpcUrl,
    #[error("Either chain_id or rpc_url must be provided")]
    MissingChainIdOrRpcUrl,
    #[error("Transaction index can not be extracted from block")]
    TransactionIndexNotFound,
    #[error("Invalid transaction version")]
    InvalidTransactionVersion,
}

impl TryFrom<SimulationRawArgs> for SimulationArgs {
    type Error = TransactionSimulationError;

    fn try_from(raw_args: SimulationRawArgs) -> Result<Self, Self::Error> {
        let mut chain_id: Option<ChainId> = None;

        let rpc_url = if let Some(chain_id_str) = raw_args.chain_id {
            let extracted_chain_id = extract_chain_id(chain_id_str.as_str())
                .map_err(|_| TransactionSimulationError::InvalidChainId)?;
            chain_id = Some(extracted_chain_id.clone());
            rpc_url(&extracted_chain_id)
        } else if let Some(rpc_url) = raw_args.rpc_url {
            Url::parse(&rpc_url).map_err(|_| TransactionSimulationError::InvalidRpcUrl)?
        } else {
            return Err(TransactionSimulationError::MissingChainIdOrRpcUrl);
        };

        let calldata: Vec<Felt> = raw_args
            .calldata
            .iter()
            .map(|x| felt!(x.as_str()))
            .collect();

        Ok(Self {
            chain_id,
            rpc_url,
            block_number: raw_args
                .block_number
                .map_or(BlockNumber::default(), BlockNumber),
            nonce: raw_args.nonce.map(|nonce| Nonce(Felt::from(nonce))),
            sender_address: contract_address!(raw_args.sender_address.as_str()),
            calldata,
            transaction_version: match raw_args.transaction_version {
                0 => TransactionVersion::ZERO,
                1 => TransactionVersion::ONE,
                2 => TransactionVersion::TWO,
                3 => TransactionVersion::THREE,
                _ => {
                    return Err(TransactionSimulationError::InvalidTransactionVersion);
                }
            },
            transaction_signature: None,
        })
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
) -> Result<(SimulationInfo, BlockTimestamp, usize), TransactionSimulationError> {
    let provider_client = create_rpc_client_from_url(args.rpc_url.clone());
    let chain_id = args.chain_id.clone();
    let (block_timestamp, transaction_index) =
        extract_block_txs_info(&provider_client, &args).await?;

    let mut cached_fork_state = CachedState::new(
        ForkStateReader::new(
            args.rpc_url.clone(),
            args.block_number.0.clone(),
            transaction_index,
        )
        .map_err(|e| {
            TransactionSimulationError::StateError(StateError::StateReadError(e.to_string()))
        })?,
    );

    let cheatnet_state = run_simulation(args, &mut cached_fork_state)?;

    let (mut simulation_info, class_hashes) =
        get_simulation_info(&cached_fork_state.state, cheatnet_state);

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
        ContractNamesFetcher::new(provider_client, chain_id.as_ref())
            .enhance_trace_with_contract_names(&mut call_trace, &mut event_trace)
            .await;
        simulation_info.call_trace = Some(call_trace);
        simulation_info.events_trace = Some(event_trace);
    }

    Ok((simulation_info, block_timestamp, transaction_index))
}

async fn extract_block_txs_info(
    provider_client: &JsonRpcClient<HttpTransport>,
    simulation_args: &SimulationArgs,
) -> Result<(BlockTimestamp, usize), TransactionSimulationError> {
    let block_id = starknet_old_types::BlockId::Number(simulation_args.block_number.0);
    let block_with_txs = provider_client.get_block_with_txs(block_id).await;
    match block_with_txs {
        Ok(starknet_old_types::MaybePendingBlockWithTxs::Block(block_txs)) => {
            let block_timestamp = BlockTimestamp(block_txs.timestamp);
            let transaction_index = extract_transaction_index(&block_txs, simulation_args);
            Ok((block_timestamp, transaction_index))
        }
        Ok(starknet_old_types::MaybePendingBlockWithTxs::PendingBlock(_)) => {
            Err(TransactionSimulationError::PendingBlock(
                "Pending block is not allowed at the configuration level".to_string(),
            ))
        }
        Err(err) => Err(TransactionSimulationError::ProviderError(err)),
    }
}

fn extract_transaction_index(
    block_with_txs: &starknet_old_types::BlockWithTxs,
    simulation_args: &SimulationArgs,
) -> usize {
    for (index, tx) in block_with_txs.transactions.iter().enumerate() {
        if match_transaction(tx, simulation_args) {
            return index;
        }
    }
    0
}

fn match_transaction(tx: &starknet_old_types::Transaction, args: &SimulationArgs) -> bool {
    let sender_address = Felt::from(*args.sender_address.0);
    let nonce = args.nonce.as_ref().map(|n| Felt::from(n.0));
    match tx {
        starknet_old_types::Transaction::Invoke(invoke_tx) => {
            match (invoke_tx, args.transaction_version.0) {
                (starknet_old_types::InvokeTransaction::V0(tx_v0), version)
                    if version == Felt::ZERO =>
                {
                    sender_address == field_element_to_felt(tx_v0.contract_address)
                        && args.calldata == vec_field_element_to_vec_felt(tx_v0.calldata.clone())
                }
                (starknet_old_types::InvokeTransaction::V1(tx_v1), version)
                    if version == Felt::ONE =>
                {
                    sender_address == field_element_to_felt(tx_v1.sender_address)
                        && args.calldata == vec_field_element_to_vec_felt(tx_v1.calldata.clone())
                        && nonce
                            .as_ref()
                            .map_or(false, |n| *n == field_element_to_felt(tx_v1.nonce))
                }
                (starknet_old_types::InvokeTransaction::V3(tx_v3), version)
                    if version == Felt::THREE =>
                {
                    sender_address == field_element_to_felt(tx_v3.sender_address)
                        && args.calldata == vec_field_element_to_vec_felt(tx_v3.calldata.clone())
                        && nonce
                            .as_ref()
                            .map_or(false, |n| *n == field_element_to_felt(tx_v3.nonce))
                }
                _ => false,
            }
        }
        starknet_old_types::Transaction::L1Handler(l1_handler_tx) => {
            let version: Felt = args.transaction_version.0;
            let l1_hanler_version: Felt = field_element_to_felt(l1_handler_tx.version);
            let _l1_handler_nonce: Felt = Felt::from(l1_handler_tx.nonce);
            version == l1_hanler_version
                && sender_address == field_element_to_felt(l1_handler_tx.contract_address)
                && args.calldata == vec_field_element_to_vec_felt(l1_handler_tx.calldata.clone())
                && nonce
                    .as_ref()
                    .map_or(false, |n| *n == Felt::from(l1_handler_tx.nonce))
        }
        starknet_old_types::Transaction::Declare(declare_tx) => {
            match (declare_tx, args.transaction_version.0) {
                (starknet_old_types::DeclareTransaction::V0(tx_v0), version)
                    if version == Felt::ZERO =>
                {
                    sender_address == field_element_to_felt(tx_v0.sender_address)
                }
                (starknet_old_types::DeclareTransaction::V1(tx_v1), version)
                    if version == Felt::ONE =>
                {
                    sender_address == field_element_to_felt(tx_v1.sender_address)
                        && nonce
                            .as_ref()
                            .map_or(false, |n| *n == field_element_to_felt(tx_v1.nonce))
                }
                (starknet_old_types::DeclareTransaction::V2(tx_v2), version)
                    if version == Felt::TWO =>
                {
                    sender_address == field_element_to_felt(tx_v2.sender_address)
                        && nonce
                            .as_ref()
                            .map_or(false, |n| *n == field_element_to_felt(tx_v2.nonce))
                }
                (starknet_old_types::DeclareTransaction::V3(tx_v3), version)
                    if version == Felt::THREE =>
                {
                    sender_address == field_element_to_felt(tx_v3.sender_address)
                        && nonce
                            .as_ref()
                            .map_or(false, |n| *n == field_element_to_felt(tx_v3.nonce))
                }
                _ => false,
            }
        }
        starknet_old_types::Transaction::Deploy(deploy_tx) => {
            let version: Felt = args.transaction_version.0;
            let deploy_version: Felt = field_element_to_felt(deploy_tx.version);
            version == deploy_version
                && args.calldata
                    == vec_field_element_to_vec_felt(deploy_tx.constructor_calldata.clone())
        }
        starknet_old_types::Transaction::DeployAccount(deploy_account_tx) => {
            match deploy_account_tx {
                starknet_old_types::DeployAccountTransaction::V1(tx_v1) => {
                    args.calldata
                        == vec_field_element_to_vec_felt(tx_v1.constructor_calldata.clone())
                        && nonce
                            .as_ref()
                            .map_or(false, |n| *n == field_element_to_felt(tx_v1.nonce))
                }
                starknet_old_types::DeployAccountTransaction::V3(tx_v3) => {
                    args.calldata
                        == vec_field_element_to_vec_felt(tx_v3.constructor_calldata.clone())
                        && nonce
                            .as_ref()
                            .map_or(false, |n| *n == field_element_to_felt(tx_v3.nonce))
                }
            }
        }
    }
}

fn run_simulation(
    args: SimulationArgs,
    cached_fork_state: &mut CachedState<ForkStateReader>,
) -> Result<CheatnetState, TransactionSimulationError> {
    let entry_point_selector = selector_from_name(constants::EXECUTE_ENTRY_POINT_NAME);

    let mut execute_call = CallEntryPoint {
        entry_point_type: EntryPointType::External,
        entry_point_selector,
        calldata: Calldata(args.calldata.into()),
        class_hash: None,
        code_address: None,
        storage_address: args.sender_address,
        caller_address: ContractAddress::default(),
        call_type: CallType::Call,
        initial_gas: u64::MAX,
    };

    let block_info = cached_fork_state.state.get_block_info()?;
    let chain_info = if let Some(chain_id) = args.chain_id {
        ChainInfo {
            chain_id,
            fee_token_addresses: FeeTokenAddresses {
                strk_fee_token_address: contract_address!(STRK_FEE_TOKEN_ADDRESS),
                eth_fee_token_address: contract_address!(ETH_FEE_TOKEN_ADDRESS),
            },
        }
    } else {
        ChainInfo::default()
    };

    let transaction_context = Arc::new(TransactionContext {
        block_context: BlockContext::new(
            block_info.clone(),
            chain_info,
            VersionedConstants::latest_constants().clone(),
            BouncerConfig::default(),
        ),
        tx_info: TransactionInfo::Current(CurrentTransactionInfo {
            common_fields: CommonAccountFields {
                transaction_hash: TransactionHash::default(),
                version: args.transaction_version,
                signature: args.transaction_signature.unwrap_or_default(),
                nonce: args.nonce.unwrap_or_default(),
                sender_address: args.sender_address,
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
        EntryPointExecutionContext::new(transaction_context, ExecutionMode::Execute, false)?;

    let mut cheatnet_state = CheatnetState {
        block_info,
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

    Ok(cheatnet_state)
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
    entry_point_function_selector: Option<String>,
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
    sierra_version: Option<String>,
    cairo_version: Option<String>,
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
    pub fn_calls: Vec<InternalFnCallTraceEntryNode>,
    pub additional_info: SimulationCallTraceAdditionalInfo,
    pub _vm_trace: Option<Vec<RelocatedTraceEntry>>,
    pub _relocated_memory: Option<Vec<Option<Felt>>>,
    pub contract_call_id: String,
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

    let first_nested_call = if call_trace_ref.nested_calls.is_empty() {
        None
    } else if let CallTraceNode::EntryPointCall(call_trace) = &call_trace_ref.nested_calls[0] {
        Some(call_trace)
    } else {
        None
    };

    let first_nested_call = match first_nested_call {
        Some(call_trace) => call_trace,
        None => {
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
    };

    let mut call_trace = get_simulation_call_trace(
        fork_state_reader,
        first_nested_call.borrow(),
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

    enhance_call_trace_with_contract_call_index(&mut call_trace, None, 0);

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

    if let CallResult::Failure(_) = &call_trace_ref.result {
        if nested_level >= *max_nested_error_level {
            *max_nested_error_level = nested_level
        }
    };

    for nested_call in &call_trace_ref.nested_calls {
        match nested_call {
            CallTraceNode::EntryPointCall(call_trace) => {
                let nested_trace = get_simulation_call_trace(
                    fork_state_reader,
                    call_trace.borrow(),
                    nested_level + 1,
                    max_nested_error_level,
                    class_hashes,
                );
                nested_calls.push(nested_trace);
            }
            CallTraceNode::DeployWithoutConstructor => {
                // TODO: explore
            }
        }
    }

    if let Some(class_hash) = call_trace_ref.entry_point.class_hash {
        class_hashes.push(class_hash.0.to_fixed_hex_string());
    }

    SimulationCallTrace {
        entry_point: call_trace_ref.entry_point.clone(),
        used_execution_resources: call_trace_ref.used_execution_resources.clone(),
        nested_calls,
        nested_level,
        result: call_trace_ref.result.clone(),
        fn_calls: Vec::new(),
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
        _relocated_memory: call_trace_ref.vm_memory.clone(),
        contract_call_id: String::new(),
    }
}

fn get_event_trace(events: &Vec<Event>, call_trace: &SimulationCallTrace) -> Vec<EventTrace> {
    let mut events_trace: Vec<EventTrace> = Vec::new();
    for event in events {
        let contract_name = event.from.to_string();

        let keys_hex = felt_vec_to_hex_vec(event.keys.to_vec());
        let mut event_name = keys_hex[0].to_string();
        let mut event_keys = Vec::new();
        if keys_hex.len() > 1 {
            event_keys = keys_hex[1..].to_vec();
        }
        let event_datas = felt_vec_to_hex_vec(event.data.to_vec());
        let event_abi = find_call_trace(call_trace, &contract_name);
        let filtered_event_abi = event_abi.as_ref().and_then(|events| {
            events
                .iter()
                .find(|abi| {
                    let selector = selector_from_name(abi.event_name.as_str());
                    selector.0.to_fixed_hex_string() == event_name
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

pub async fn simulate_by_data(
    db_pool: &Pool<Postgres>,
    s3_client: &aws_sdk_s3::Client,
    args: SimulationArgs,
) -> Result<TransactionSimulationResult, TransactionSimulationError> {
    let nonce: Option<u64> = match args.nonce {
        Some(nonce) => nonce.0.to_u64(),
        None => None,
    };
    let readable_chain_id = args
        .chain_id
        .clone()
        .map(|chain_id| chain_id_to_readable_string(&chain_id));
    let block_number = args.block_number.0;
    let sender_address = args.sender_address.0.to_string();
    let calldata = args
        .calldata
        .iter()
        .map(|x| x.to_string())
        .collect::<Vec<String>>();

    let transaction_version: usize = args.transaction_version.0.to_u64().unwrap() as usize;
    let (simulation_result, block_timestamp, transaction_index_in_block) =
        simulate(db_pool, s3_client, args).await?;

    Ok(TransactionSimulationResult {
        simulation_result,
        chain_id: readable_chain_id,
        block_number,
        block_timestamp: block_timestamp.0,
        transaction_index_in_block,
        nonce,
        sender_address,
        calldata,
        transaction_version,
        transaction_type: transaction_type_to_string(TransactionType::InvokeFunction),
    })
}

pub async fn simulate_transaction_by_hash(
    db_pool: &Pool<Postgres>,
    s3_client: &aws_sdk_s3::Client,
    rpc_url: Url,
    tx_hash: String,
    chain_id: Option<ChainId>,
) -> Result<TransactionSimulationResult, TransactionSimulationError> {
    let provider_client = create_rpc_client_from_url(rpc_url.clone());
    let transaction_hash = Felt::from_str(tx_hash.as_str()).unwrap();
    let transaction = provider_client
        .get_transaction_by_hash(felt_to_field_element(transaction_hash))
        .await;
    if let Ok(transaction) = transaction {
        if let Some((
            nonce,
            sender_address,
            calldata,
            transaction_version,
            transaction_type,
            signature,
        )) = extract_submitted_tx(transaction)
        {
            let transaction_receipt = provider_client
                .get_transaction_receipt(felt_to_field_element(transaction_hash))
                .await;

            if let Ok(transaction_receipt) = transaction_receipt {
                if let Some(block_number) = extract_transaction_receipt(transaction_receipt) {
                    let (simulation_result, block_timestamp, transaction_index_in_block) =
                        simulate(
                            db_pool,
                            s3_client,
                            SimulationArgs {
                                rpc_url,
                                chain_id: chain_id.clone(),
                                block_number: BlockNumber(block_number),
                                nonce: Some(nonce),
                                sender_address,
                                calldata: calldata.0.to_vec(),
                                transaction_version,
                                transaction_signature: Some(signature),
                            },
                        )
                        .await?;
                    let calldata = calldata
                        .0
                        .iter()
                        .map(|x| x.to_string())
                        .collect::<Vec<String>>();
                    let nonce = nonce.0.to_u64();
                    return Ok(TransactionSimulationResult {
                        simulation_result,
                        chain_id: chain_id.map(|id| chain_id_to_readable_string(&id)),
                        block_number,
                        block_timestamp: block_timestamp.0,
                        transaction_index_in_block,
                        nonce,
                        sender_address: sender_address.0.to_string(),
                        calldata,
                        transaction_version: transaction_version.0.to_u64().unwrap() as usize,
                        transaction_type: transaction_type_to_string(transaction_type),
                    });
                }
            }
        }
    }
    Err(TransactionSimulationError::TransactionHashNotFound)
}

fn extract_transaction_receipt(
    transaction_receipt: starknet_old_types::MaybePendingTransactionReceipt,
) -> Option<u64> {
    match transaction_receipt {
        starknet_old_types::MaybePendingTransactionReceipt::Receipt(receipt) => match receipt {
            starknet_old_types::TransactionReceipt::Invoke(invoke_receipt) => {
                Some(invoke_receipt.block_number)
            }
            starknet_old_types::TransactionReceipt::Declare(declare_receipt) => {
                Some(declare_receipt.block_number)
            }
            _ => None,
        },
        _ => None,
    }
}

fn extract_submitted_tx(
    transaction: starknet_old_types::Transaction,
) -> Option<(
    Nonce,
    ContractAddress,
    Calldata,
    TransactionVersion,
    TransactionType,
    TransactionSignature,
)> {
    match transaction {
        starknet_old_types::Transaction::Invoke(invoke_transaction) => match invoke_transaction {
            starknet_old_types::InvokeTransaction::V0(tx) => Some((
                Nonce::default(),
                field_element_to_felt(tx.contract_address)
                    .try_into()
                    .unwrap(),
                Calldata(vec_field_element_to_vec_felt(tx.calldata).into()),
                TransactionVersion::ZERO,
                TransactionType::InvokeFunction,
                TransactionSignature(vec_field_element_to_vec_felt(tx.signature).into()),
            )),
            starknet_old_types::InvokeTransaction::V1(tx) => Some((
                Nonce(field_element_to_felt(tx.nonce)),
                field_element_to_felt(tx.sender_address).try_into().unwrap(),
                Calldata(vec_field_element_to_vec_felt(tx.calldata).into()),
                TransactionVersion::ONE,
                TransactionType::InvokeFunction,
                TransactionSignature(vec_field_element_to_vec_felt(tx.signature).into()),
            )),
            starknet_old_types::InvokeTransaction::V3(tx) => Some((
                Nonce(field_element_to_felt(tx.nonce)),
                field_element_to_felt(tx.sender_address).try_into().unwrap(),
                Calldata(vec_field_element_to_vec_felt(tx.calldata).into()),
                TransactionVersion::THREE,
                TransactionType::InvokeFunction,
                TransactionSignature(vec_field_element_to_vec_felt(tx.signature).into()),
            )),
        },
        starknet_old_types::Transaction::Declare(declare_transaction) => {
            match declare_transaction {
                starknet_old_types::DeclareTransaction::V0(tx) => Some((
                    Nonce::default(),
                    field_element_to_felt(tx.sender_address).try_into().unwrap(),
                    Calldata::default(),
                    TransactionVersion::ZERO,
                    TransactionType::Declare,
                    TransactionSignature(vec_field_element_to_vec_felt(tx.signature).into()),
                )),
                starknet_old_types::DeclareTransaction::V1(tx) => Some((
                    Nonce(field_element_to_felt(tx.nonce)),
                    field_element_to_felt(tx.sender_address).try_into().unwrap(),
                    Calldata::default(),
                    TransactionVersion::ONE,
                    TransactionType::Declare,
                    TransactionSignature(vec_field_element_to_vec_felt(tx.signature).into()),
                )),
                starknet_old_types::DeclareTransaction::V2(tx) => Some((
                    Nonce(field_element_to_felt(tx.nonce)),
                    field_element_to_felt(tx.sender_address).try_into().unwrap(),
                    Calldata::default(),
                    TransactionVersion::TWO,
                    TransactionType::Declare,
                    TransactionSignature(vec_field_element_to_vec_felt(tx.signature).into()),
                )),
                starknet_old_types::DeclareTransaction::V3(tx) => Some((
                    Nonce(field_element_to_felt(tx.nonce)),
                    field_element_to_felt(tx.sender_address).try_into().unwrap(),
                    Calldata::default(),
                    TransactionVersion::THREE,
                    TransactionType::Declare,
                    TransactionSignature(vec_field_element_to_vec_felt(tx.signature).into()),
                )),
            }
        }
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
        entry_point_function_selector: None,
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
        class_hash: class_hash.map(|class_hash| class_hash.0.to_fixed_hex_string()),
        sierra_version: None,
        cairo_version: None,
    };
    let mut struct_items: Vec<StructItems> = Vec::new();
    let mut enum_items: Vec<EnumItems> = Vec::new();
    let mut event_items: Vec<EventItems> = Vec::new();
    if let Some(class_hash) = class_hash {
        let contract_class = fork_state_reader
            .in_memory_fork_cache
            .borrow()
            .get_contract_class(class_hash)
            .ok();
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
                    enum_items = abi_processor.enum_items;
                    event_items = abi_processor.event_items;
                    let (sierra_version, cairo_version) = extract_version(&class.sierra_program);
                    additional_info.sierra_version = sierra_version;
                    additional_info.cairo_version = cairo_version;
                }
                _ => {}
            };
        }
    }

    get_event_data(&mut additional_info, &event_items);
    get_function_name(&mut additional_info, &entry_point_selector);
    get_function_result(&mut additional_info, &result, &struct_items, &enum_items);
    get_function_arguments(&mut additional_info, &calldata, &struct_items, &enum_items);

    additional_info
}

fn extract_version(sierra_program: &[Felt]) -> (Option<String>, Option<String>) {
    if sierra_program.len() < 6 {
        return (None, None);
    }
    let sierra_version = format!(
        "{}.{}.{}",
        sierra_program[0], sierra_program[1], sierra_program[2]
    );

    let cairo_version = format!(
        "{}.{}.{}",
        sierra_program[3], sierra_program[4], sierra_program[5]
    );

    (Some(sierra_version), Some(cairo_version))
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
    additional_info.entry_point_function_selector = Some(entry_point_selector.0.to_string());
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
    enum_items: &Vec<EnumItems>,
) {
    if let CallResult::Success { ret_data } = call_result {
        let ret_hex = felt_vec_to_hex_vec(ret_data.to_vec());
        if let Some(function_return_result_types) =
            additional_info.function_return_result_types.clone()
        {
            let decoded_result = decode_datas(
                &ret_hex,
                &function_return_result_types,
                &vec![],
                Some(struct_items),
                Some(enum_items),
                &mut 0,
            );
            additional_info.function_result = Some(json!(decoded_result));
        }
    }
}

fn get_function_arguments(
    additional_info: &mut SimulationCallTraceAdditionalInfo,
    calldata: &Calldata,
    struct_items: &Vec<StructItems>,
    enum_items: &Vec<EnumItems>,
) {
    if let (Some(function_arguments_types), Some(function_arguments_names)) = (
        additional_info.function_arguments_types.clone(),
        additional_info.function_arguments_names.clone(),
    ) {
        let calldata_hex: Vec<String> = calldata.0.iter().map(|x| x.to_hex_string()).collect();
        let decoded_arguments = decode_datas(
            &calldata_hex,
            &function_arguments_types,
            &function_arguments_names,
            Some(struct_items),
            Some(enum_items),
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
                    CallFailure::Panic { panic_data } => match decode_felt(panic_data.to_vec()) {
                        Ok(decoded) => {
                            nested_trace.additional_info.error_message = Some(decoded.clone());
                            *execution_result = ExecutionResult::Reverted { reason: decoded };
                        }
                        Err(_) => panic!("Failed to decode felt252"),
                    },
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
    let parent_contract_call_id = simulation_call_trace.contract_call_id.clone();
    let (internal_fn_call_trace, call_debugger_data) = match (
        simulation_call_trace.entry_point.class_hash,
        &simulation_call_trace._relocated_memory,
        &simulation_call_trace._vm_trace,
    ) {
        (Some(class_hash), Some(relocated_memory), Some(vm_trace)) => {
            match classes_debugger_data.get(&class_hash.0.to_fixed_hex_string()) {
                Some(full_class_debugger_data) => {
                    match get_internal_trace_and_debugger_data(
                        relocated_memory,
                        vm_trace,
                        full_class_debugger_data,
                        &parent_contract_call_id,
                    ) {
                        Ok((internal_fn_call_trace, call_debugger_data)) => {
                            (Some(internal_fn_call_trace), Some(call_debugger_data))
                        }
                        Err(e) => {
                            error!("Failed to get internal fn call trace: {:?}", e);
                            (None, None)
                        }
                    }
                }
                None => (None, None),
            }
        }
        _ => {
            error!("Not enough data to get internal fn call trace");
            (None, None)
        }
    };
    simulation_call_trace.fn_calls =
        internal_fn_call_trace.map_or_else(Vec::new, |trace| vec![trace]);
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

fn enhance_call_trace_with_contract_call_index(
    simulation_call_trace: &mut SimulationCallTrace,
    parent_contract_call_index: Option<&str>,
    current_contract_call_index: usize,
) {
    simulation_call_trace.contract_call_id =
        get_contract_call_id(parent_contract_call_index, current_contract_call_index);
    for (index, nested_call) in simulation_call_trace.nested_calls.iter_mut().enumerate() {
        enhance_call_trace_with_contract_call_index(
            nested_call,
            Some(&simulation_call_trace.contract_call_id),
            index,
        );
    }
}
