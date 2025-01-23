use blockifier::abi::abi_utils::selector_from_name;
use blockifier::blockifier::block::BlockInfo;
use blockifier::context::TransactionContext;
use blockifier::execution::call_info::CallInfo;
use blockifier::execution::call_info::Retdata;
use blockifier::execution::common_hints::ExecutionMode;
use blockifier::execution::contract_class::ContractClass as BlockifierContractClass;
use blockifier::execution::entry_point::CallEntryPoint;
use blockifier::execution::entry_point::CallType;
use blockifier::execution::entry_point::EntryPointExecutionContext;
use blockifier::retdata;
use blockifier::state::cached_state::CachedState;
use blockifier::state::errors::StateError;
use blockifier::state::state_api::State;
use blockifier::transaction::constants;
use blockifier::transaction::errors::TransactionExecutionError;
use blockifier::transaction::transaction_types::TransactionType;
use cairo_vm::vm::runners::cairo_runner::ExecutionResources;
use cheatnet::runtime_extensions::call_to_blockifier_runtime_extension::execution::entry_point::execute_call_entry_point;
use cheatnet::runtime_extensions::call_to_blockifier_runtime_extension::rpc::CallFailure;
use cheatnet::runtime_extensions::call_to_blockifier_runtime_extension::rpc::CallResult;
use cheatnet::runtime_extensions::forge_runtime_extension::cheatcodes::spy_events::Event;
use cheatnet::state::BlockInfoReader;
use cheatnet::state::CheatnetState;
use internal_tracing::build_debugger_data::debugger_data_maps_full_class_to_class;
use internal_tracing::debugger_data_fetcher::fetch_classes_debugger_data;
use internal_tracing::SimulationDebuggerData;
use num_traits::ToPrimitive;
use sqlx::Pool;
use sqlx::Postgres;
use starknet::core::types::ContractClass;
use starknet::core::types::ExecutionResult;
use starknet::core::types::Felt;
use starknet_api::block;
use starknet_api::block::BlockNumber;
use starknet_api::block::BlockTimestamp;
use starknet_api::core::EntryPointSelector;
use starknet_api::core::{ChainId, ContractAddress};
use starknet_api::deprecated_contract_class::EntryPointType;
use starknet_api::transaction::{Calldata, TransactionHash};
use starknet_old::core::types as starknet_old_types;
use starknet_providers::Provider;
use std::collections::HashMap;
use std::sync::Arc;
use tracing::warn;
use url::Url;
use walnut_shared::felt_to_field_element;
use walnut_shared::felt_vec_to_hex_vec;
use walnut_shared::felts_to_string;
use walnut_shared::field_element_to_felt;
use walnut_shared::{chain_id_to_readable_string, create_rpc_client_from_url};

use crate::abi_processor::AbiProcessor;
use crate::contract_calls_map::ContractCallsMap;
use crate::contract_calls_map::ContractCallsMapBuilder;
use crate::contract_names::ContractNamesFetcher;
use crate::debugger_trace::DebuggerTraceBuilder;
use crate::function_calls::create_function_calls_map;
use crate::state::ForkStateReader;
use crate::transaction_extraction::extract_block_number_transaction_receipt;
use crate::transaction_extraction::extract_block_timestamp;
use crate::transaction_extraction::extract_block_txs_info;
use crate::transaction_extraction::extract_chain_id_from_felt;
use crate::transaction_extraction::extract_execution_status_transaction_receipt;
use crate::transaction_extraction::extract_submitted_tx;
use crate::transaction_extraction::extract_transaction_contex;
use crate::utils::calldata_to_hex;
use crate::utils::parse_transaction_hash;
use crate::utils::transaction_type_to_string;
use crate::ContractCall;
use crate::ContractCallEvent;
use crate::EventAbi;
use crate::FunctionCallsMap;
use crate::SimulationArgs;
use crate::SimulationInfo;
use crate::TransactionSimulationError;
use crate::TransactionSimulationResult;

pub async fn simulate(
    db_pool: &Pool<Postgres>,
    s3_client: &aws_sdk_s3::Client,
    args: SimulationArgs,
) -> Result<(SimulationInfo, BlockTimestamp, usize), TransactionSimulationError> {
    let provider_client = create_rpc_client_from_url(args.rpc_url.clone());
    let chain_id = args.chain_id.clone();
    let block_number = if let Some(bn) = args.block_number {
        bn.0
    } else {
        provider_client.block_number().await?
    };
    let (block_info, transaction_index, tx_number_in_block) =
        extract_block_txs_info(&provider_client, &args, block_number).await?;

    let block_timestamp = block_info.block_timestamp;
    let mut cached_fork_state = CachedState::new(
        ForkStateReader::new(
            args.rpc_url.clone(),
            block_number,
            transaction_index,
            tx_number_in_block,
        )
        .map_err(|e| {
            TransactionSimulationError::StateError(StateError::StateReadError(e.to_string()))
        })?,
    );

    let cheatnet_state = run_simulation(block_info, args, &mut cached_fork_state)?;

    let ContractCallsMapBuilder {
        mut contract_calls_map,
        next_call_id,
        deepest_failed_contract_call_id,
        cheatnet_state_detected_events,
        ..
    } = ContractCallsMapBuilder::new_from_cheatnet_state(cheatnet_state);

    let class_hashes = contract_calls_map.collect_all_class_hashes();

    let classes_debugger_data = fetch_classes_debugger_data(db_pool, s3_client, class_hashes).await;

    let mut function_calls_map = create_function_calls_map(
        &mut contract_calls_map,
        next_call_id,
        &classes_debugger_data,
    );

    filter_and_hide_unlinked_function_calls(&mut contract_calls_map, &function_calls_map);

    let mut event_abis: Vec<EventAbi> = Vec::new();

    for call in contract_calls_map.0.values_mut() {
        if let Some(class_hash) = call.entry_point.class_hash {
            let contract_class = cached_fork_state
                .state
                .in_memory_fork_cache
                .borrow()
                .get_contract_class(class_hash)
                .ok();
            if let Some(ContractClass::Sierra(class)) = contract_class {
                let mut abi_processor = AbiProcessor::new(call.entry_point.entry_point_selector);
                abi_processor.process_abi(class.abi);
                call.entry_point_name = abi_processor.entry_point_function_name;
                call.entry_point_interface_name = abi_processor.entry_point_interface_name;
                call.is_erc20_token = abi_processor.is_erc20_token;
                call.arguments_names = abi_processor.function_arguments_names;
                call.arguments_types = abi_processor.function_arguments_types;
                call.result_types = abi_processor.function_return_result_types;
                event_abis.extend(abi_processor.event_abis.into_iter());
                let (sierra_version, cairo_version) =
                    extract_sierra_and_cairo_versions(&class.sierra_program);
                call.sierra_version = sierra_version;
                call.cairo_version = cairo_version;
                call.decode_call_result(&abi_processor.struct_abis, &abi_processor.enum_abis);
                call.decode_call_arguments(&abi_processor.struct_abis, &abi_processor.enum_abis)
            }
        }
    }

    let events = get_events_from_cheatnet_state(
        cheatnet_state_detected_events,
        &event_abis,
        &contract_calls_map,
    );

    ContractNamesFetcher::new(provider_client, &chain_id)
        .set_contract_names(&mut contract_calls_map)
        .await;

    let execution_result =
        get_execution_result(&contract_calls_map.0, deepest_failed_contract_call_id)?;

    if let ExecutionResult::Reverted { reason, .. } = &execution_result {
        if let Some(call) = contract_calls_map
            .0
            .get_mut(&deepest_failed_contract_call_id.unwrap())
        {
            call.error_message = Some(reason.clone());
            call.is_deepest_panic_result = true;
        }
    }

    let debugger_trace =
        DebuggerTraceBuilder::build(&1, &mut function_calls_map, &mut contract_calls_map);

    let simulation_info = SimulationInfo {
        contract_calls_map,
        function_calls_map,
        events,
        execution_result,
        simulation_debugger_data: Some(SimulationDebuggerData {
            classes_debugger_data: debugger_data_maps_full_class_to_class(classes_debugger_data),
            debugger_trace,
        }),
    };

    Ok((simulation_info, block_timestamp, transaction_index))
}

fn filter_and_hide_unlinked_function_calls(
    contract_calls_map: &mut ContractCallsMap,
    function_calls_map: &FunctionCallsMap,
) {
    for contract_call in contract_calls_map.0.values_mut().filter(|c| !c.is_hidden) {
        for &child_id in &contract_call.children_call_ids {
            if contract_call.function_call_id.is_some()
                && !function_calls_map
                    .0
                    .values()
                    .any(|fc| fc.children_call_ids.contains(&child_id))
            {
                warn!("Hide function calls of the contract {:?} that has the contract call to the one that was not added by decoding system calls.",
                    contract_call.entry_point.storage_address);
                contract_call.function_call_id = None;
            }
        }
    }
}

fn run_simulation(
    block_info: BlockInfo,
    args: SimulationArgs,
    cached_fork_state: &mut CachedState<ForkStateReader>,
) -> Result<CheatnetState, TransactionSimulationError> {
    let transaction_context = extract_transaction_contex(
        &args.sender_address,
        &args.transaction_version,
        args.transaction_signature,
        &args.transaction_hash,
        &args.nonce,
        args.chain_id,
        &block_info,
        args.resource_bounds,
        args.paymaster_data,
    );

    let mut cheatnet_state = CheatnetState {
        block_info,
        ..Default::default()
    };

    cheatnet_state.trace_data.is_vm_trace_needed = true;

    if let Some(_transaction_hash) = args.transaction_hash {
        let validate_selector = match args.transaction_type {
            Some(TransactionType::Declare) => {
                selector_from_name(constants::VALIDATE_DECLARE_ENTRY_POINT_NAME)
            }
            _ => selector_from_name(constants::VALIDATE_ENTRY_POINT_NAME),
        };

        let _validate_result = validate_call(
            args.calldata.clone(),
            args.sender_address,
            validate_selector,
            cached_fork_state,
            &mut cheatnet_state,
            transaction_context.clone(),
            u64::MAX,
        );
    }

    if args.transaction_type.is_none() || args.transaction_type != Some(TransactionType::Declare) {
        let _execution_result = execute_call(
            args.calldata.clone(),
            args.sender_address,
            cached_fork_state,
            &mut cheatnet_state,
            transaction_context.clone(),
            u64::MAX,
        );
    }

    Ok(cheatnet_state)
}

pub async fn simulate_by_calldata(
    db_pool: &Pool<Postgres>,
    s3_client: &aws_sdk_s3::Client,
    args: SimulationArgs,
) -> Result<TransactionSimulationResult, TransactionSimulationError> {
    let nonce: Option<u64> = match args.nonce {
        Some(nonce) => nonce.0.to_u64(),
        None => None,
    };
    let readable_chain_id = chain_id_to_readable_string(&args.chain_id);
    let block_number = if let Some(bn) = args.block_number {
        starknet_old_types::BlockId::Number(bn.0)
    } else {
        starknet_old_types::BlockId::Tag(starknet_old_types::BlockTag::Latest)
    };

    let sender_address = args.sender_address.0.to_string();
    let calldata = args
        .calldata
        .0
        .to_vec()
        .iter()
        .map(|felt| felt.to_hex_string())
        .collect::<Vec<String>>();
    let transaction_version: usize = args.transaction_version.0.to_u64().unwrap() as usize;
    let (simulation_result, block_timestamp, transaction_index_in_block) =
        simulate(db_pool, s3_client, args).await?;

    Ok(TransactionSimulationResult {
        simulation_result,
        chain_id: readable_chain_id,
        block_number,
        block_timestamp: block_timestamp.0,
        transaction_index_in_block: Some(transaction_index_in_block),
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
    tx_hash: &str,
    chain_id: Option<ChainId>,
) -> Result<TransactionSimulationResult, TransactionSimulationError> {
    let provider_client = create_rpc_client_from_url(rpc_url.clone());
    let transaction_hash = parse_transaction_hash(tx_hash)?;
    // Fetch transaction details
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
            resource_bounds,
            paymaster_data,
        )) = extract_submitted_tx(transaction)
        {
            // Fetch receipt
            let transaction_receipt = provider_client
                .get_transaction_receipt(felt_to_field_element(transaction_hash))
                .await;
            if let Ok(transaction_receipt) = transaction_receipt {
                //Fetch block number
                if let Some(block_number) =
                    extract_block_number_transaction_receipt(&transaction_receipt)
                {
                    // Fetch or derive chain_id
                    let chain_id = match chain_id {
                        Some(chain_id) => chain_id,
                        None => extract_chain_id_from_felt(field_element_to_felt(
                            provider_client
                                .chain_id()
                                .await
                                .map_err(|_| TransactionSimulationError::FailedToFetchChainId)?,
                        ))?,
                    };

                    // Check for execution status
                    if let Some(starknet_old_types::ExecutionResult::Reverted { reason }) =
                        extract_execution_status_transaction_receipt(&transaction_receipt)
                    {
                        if reason.contains("RunResources") {
                            // Fetch block timestamp
                            let block_timestamp =
                                extract_block_timestamp(&provider_client, block_number).await?;

                            let simulation_info = SimulationInfo {
                                contract_calls_map: ContractCallsMap::new(),
                                function_calls_map: FunctionCallsMap::new(),
                                events: Vec::new(),
                                execution_result: ExecutionResult::Reverted { reason },
                                simulation_debugger_data: Some(SimulationDebuggerData {
                                    classes_debugger_data: HashMap::new(),
                                    debugger_trace: Vec::new(),
                                }),
                            };
                            return Ok(TransactionSimulationResult {
                                simulation_result: simulation_info,
                                chain_id: chain_id_to_readable_string(&chain_id),
                                block_number: starknet_old_types::BlockId::Number(block_number),
                                block_timestamp: block_timestamp.0,
                                transaction_index_in_block: None,
                                nonce: nonce.0.to_u64(),
                                sender_address: sender_address.0.to_string(),
                                calldata: calldata_to_hex(&calldata),
                                transaction_version: transaction_version.0.to_u64().unwrap()
                                    as usize,
                                transaction_type: transaction_type_to_string(transaction_type),
                            });
                        }
                    }

                    // Perform transaction simulation
                    let (simulation_result, block_timestamp, transaction_index_in_block) =
                        simulate(
                            db_pool,
                            s3_client,
                            SimulationArgs {
                                rpc_url,
                                chain_id: chain_id.clone(),
                                block_number: Some(BlockNumber(block_number)),
                                nonce: Some(nonce),
                                sender_address,
                                calldata: calldata.clone(),
                                transaction_version,
                                transaction_signature: Some(signature),
                                transaction_hash: Some(TransactionHash(transaction_hash)),
                                transaction_type: Some(transaction_type),
                                resource_bounds: Some(resource_bounds),
                                paymaster_data: Some(paymaster_data),
                            },
                        )
                        .await?;

                    // Build and return simulation result
                    return Ok(TransactionSimulationResult {
                        simulation_result,
                        chain_id: chain_id_to_readable_string(&chain_id),
                        block_number: starknet_old_types::BlockId::Number(block_number),
                        block_timestamp: block_timestamp.0,
                        transaction_index_in_block: Some(transaction_index_in_block),
                        nonce: nonce.0.to_u64(),
                        sender_address: sender_address.0.to_string(),
                        calldata: calldata_to_hex(&calldata),
                        transaction_version: transaction_version.0.to_u64().unwrap() as usize,
                        transaction_type: transaction_type_to_string(transaction_type),
                    });
                }
            }
        }
    }
    Err(TransactionSimulationError::TransactionHashNotFound)
}

fn extract_sierra_and_cairo_versions(sierra_program: &[Felt]) -> (Option<String>, Option<String>) {
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

// TODO
fn get_events_from_cheatnet_state(
    cheatnet_state_detected_events: Vec<Event>,
    event_abis: &[EventAbi],
    contract_calls_map: &ContractCallsMap,
) -> Vec<ContractCallEvent> {
    let mut events: Vec<ContractCallEvent> = Vec::new();
    for cheatnet_state_event in cheatnet_state_detected_events {
        let event_selector = cheatnet_state_event.keys[0];
        let event_data_hex = felt_vec_to_hex_vec(cheatnet_state_event.data.to_vec());
        let event_abi = event_abis.iter().find(|abi| {
            let selector = selector_from_name(&abi.name).0;
            selector == event_selector
        });

        let contract_call = contract_calls_map
            .0
            .values()
            .find(|call| call.entry_point.storage_address == cheatnet_state_event.from)
            .unwrap();

        if let Some(event_abi) = event_abi {
            let event = ContractCallEvent {
                contract_call_id: contract_call.call_id,
                name: event_abi.name.clone(),
                keys: felt_vec_to_hex_vec(cheatnet_state_event.keys.to_vec()),
                parameters: event_abi.parameters.clone(),
                data: event_data_hex,
            };
            events.push(event);
        }
    }

    events
}

fn get_execution_result(
    contract_calls_map: &HashMap<u32, ContractCall>,
    deepest_contract_call_id: Option<u32>,
) -> Result<ExecutionResult, TransactionSimulationError> {
    if let Some(deepest_contract_call_id) = deepest_contract_call_id {
        if let Some(call) = contract_calls_map.get(&deepest_contract_call_id) {
            if let CallResult::Failure(failure) = &call.result {
                match failure {
                    CallFailure::Panic { panic_data } => {
                        let decoded_strings = felts_to_string(panic_data);
                        let reason = decoded_strings.join(" ");

                        Ok(ExecutionResult::Reverted { reason })
                    }
                    CallFailure::Error { msg } => Ok(ExecutionResult::Reverted {
                        reason: msg.to_string(),
                    }),
                }
            } else {
                Ok(ExecutionResult::Succeeded)
            }
        } else {
            unreachable!("deepest_contract_call_id not found in contract_calls_map");
        }
    } else {
        Ok(ExecutionResult::Succeeded)
    }
}

fn validate_call(
    calldata: Calldata,
    storage_address: ContractAddress,
    validate_selector: EntryPointSelector,
    state: &mut dyn State,
    cheatnet_state: &mut CheatnetState,
    tx_context: Arc<TransactionContext>,
    initial_gas: u64,
) -> Result<CallInfo, TransactionSimulationError> {
    let mut resources = ExecutionResources::default();

    let mut validation_context =
        EntryPointExecutionContext::new(tx_context.clone(), ExecutionMode::Validate, false)?;

    let class_hash = state.get_class_hash_at(storage_address)?;

    let mut validate_call = CallEntryPoint {
        entry_point_type: EntryPointType::External,
        entry_point_selector: validate_selector,
        calldata,
        class_hash: None,
        code_address: None,
        storage_address,
        caller_address: ContractAddress::default(),
        call_type: CallType::Call,
        initial_gas,
    };

    let validate_call_info = execute_call_entry_point(
        &mut validate_call,
        state,
        cheatnet_state,
        &mut resources,
        &mut validation_context,
    );

    let validate_call_info = match validate_call_info {
        Ok(info) => info,
        Err(err) => {
            return Err(TransactionSimulationError::TransactionExecutionError(
                TransactionExecutionError::ExecutionError {
                    error: err,
                    class_hash,
                    storage_address,
                    selector: validate_selector,
                },
            ));
        }
    };
    let contract_class = state.get_compiled_contract_class(class_hash)?;
    if matches!(
        contract_class,
        BlockifierContractClass::V0(_) | BlockifierContractClass::V1(_)
    ) {
        let expected_retdata = retdata![Felt::from_hex(constants::VALIDATE_RETDATA)?];

        if validate_call_info.execution.retdata != expected_retdata {
            return Err(TransactionSimulationError::TransactionExecutionError(
                TransactionExecutionError::InvalidValidateReturnData {
                    actual: validate_call_info.execution.retdata,
                },
            ));
        }
    }

    Ok(validate_call_info)
}

fn execute_call(
    calldata: Calldata,
    storage_address: ContractAddress,
    state: &mut dyn State,
    cheatnet_state: &mut CheatnetState,
    tx_context: Arc<TransactionContext>,
    initial_gas: u64,
) -> Result<CallInfo, TransactionSimulationError> {
    let mut resources = ExecutionResources::default();
    let mut execution_context =
        EntryPointExecutionContext::new(tx_context.clone(), ExecutionMode::Execute, false)?;

    let entry_point_selector = selector_from_name(constants::EXECUTE_ENTRY_POINT_NAME);

    let mut execute_call = CallEntryPoint {
        entry_point_type: EntryPointType::External,
        entry_point_selector,
        calldata,
        class_hash: None,
        code_address: None,
        storage_address,
        caller_address: ContractAddress::default(),
        call_type: CallType::Call,
        initial_gas,
    };

    let execution_result = execute_call_entry_point(
        &mut execute_call,
        state,
        cheatnet_state,
        &mut resources,
        &mut execution_context,
    )?;

    Ok(execution_result)
}
