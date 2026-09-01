// These simulation entry points share the same wide set of inputs.
#![allow(clippy::too_many_arguments)]

// What a full simulation returns: info, block timestamp, two counts,
// optional flame charts (full and trimmed), and execution resources.
type SimulationOutput = (
    SimulationInfo,
    BlockTimestamp,
    usize,
    usize,
    Option<FlameChartNode>,
    Option<FlameChartNode>,
    Option<ExecutionResources>,
);

use crate::contract_calls_map::ContractCallsMap;
use crate::contract_calls_map::ContractCallsMapBuilder;
use crate::contract_names::ContractNamesFetcher;
use crate::convert_entry_point_error_with_block;
use crate::events::EmittedEvent;
use crate::execution::execute_transaction_flows_with_executor;
use crate::execution::PostExecStateData;
use crate::execution::{get_execution_result, handle_post_exec_and_collect_gas_vectors};
use crate::flamegraph::{build_flamegraph, build_l1_data_flamegraph};
use crate::function_calls::create_function_calls_map_generic;
use crate::gas_counter::GasCounter;
use crate::state::ForkStateReader;
use crate::storage_changes::fetch_before_storage_changes;
use crate::transaction_extraction::extract_actual_fee_transaction_receipt;
use crate::transaction_extraction::extract_block_timestamp;
use crate::transaction_extraction::extract_block_txs_info;
use crate::transaction_extraction::extract_chain_id_from_felt;
use crate::transaction_extraction::extract_execution_resources_transaction_receipt;
use crate::transaction_extraction::extract_execution_status_transaction_receipt;
use crate::transaction_extraction::extract_starkgate_event_transaction_receipt;
use crate::transaction_extraction::extract_submitted_tx;
use crate::transaction_extraction::extract_transaction_contex;
use crate::utils::{calldata_to_hex, transaction_type_to_string};
use crate::DecodedL2ToL1Message;
use crate::EStarknetL1L2Event;
use crate::EStarknetL2L1Event;
use crate::FlameChartNode;
use crate::FunctionCallsMap;
use crate::L1TransactionData;
use crate::L2TransactionData;
use crate::SimulationArgs;
use crate::SimulationInfo;
use crate::TransactionSimulationError;
use crate::TransactionSimulationResult;
use blockifier::execution::errors::AnnotatedEntryPointExecutionError;
use blockifier::state::cached_state::CachedState;
use blockifier::state::errors::StateError;
use cheatnet::runtime_extensions::outer_call_runtime_extension::execution::entry_point::{
    execute_call_entry_point, ExecuteCallEntryPointExtraOptions,
};
use cheatnet::state::CheatnetState;
use ethers::abi::AbiDecode;
use ethers::abi::AbiEncode;
use ethers::providers::Middleware;
use ethers::types::Address;
use ethers::types::Log;
use ethers::types::TransactionReceipt as EthTransactionReceipt;
use ethers::types::H160;
use ethers::types::H256;
use ethers::types::U256;
use ethers::utils::hex::hex;
use ethers::utils::keccak256;
use futures::future;
use internal_tracing::background_retry::BackgroundRetryService;
use internal_tracing::build_debugger_data::build_simple_contract_call_debugger_data_adapter;
use internal_tracing::debugger_data_fetcher::{check_voyager_verified_classes, fetch_classes_data};
use internal_tracing::event_calls_map::EventCallsMap;
use internal_tracing::external_class_cache::ExternalClassCache;
use internal_tracing::DataWithContractClass;
use internal_tracing::SimulationDebuggerData;
use num_traits::ToPrimitive;
use sqlx::Pool;
use sqlx::Postgres;
use starknet_api::block::BlockInfo;
use starknet_api::block::BlockNumber;
use starknet_api::block::BlockTimestamp;
use starknet_api::core::ChainId;
use starknet_api::executable_transaction::TransactionType;
use starknet_api::transaction::L1HandlerTransaction;
use starknet_api::transaction::{TransactionHash, TransactionHasher, TransactionVersion};
use starknet_rust::core::types::{
    BlockId, ContractClass, ExecutionResources, ExecutionResult, Felt, ReceiptBlock, Transaction,
};
use starknet_rust::providers::Provider;
use std::collections::HashMap;
use std::collections::HashSet;
use std::convert::TryFrom;
use std::time::Duration;
use tracing::{debug, info, instrument, warn};
use url::Url;
use verification::voyager::VoyagerClient;
use walnut_shared::abi::{Enum, Event, Struct};
use walnut_shared::abi_processor::AbiProcessor;
use walnut_shared::fetch_tx_block_number_from_voyager;
use walnut_shared::mask_private_token_in_url;
use walnut_shared::parse_transaction_hash_per_network;
use walnut_shared::utils::extract_sierra_and_cairo_versions;
use walnut_shared::{
    chain_id_to_readable_string, chain_id_to_url_format, create_eth_provider_from_url,
    create_rpc_client_from_url, to_chain_id, EChainId, ENetwork, ETransactionHashType,
};

const VOYAGER_PHASE_1_TIMEOUT: Duration = Duration::from_secs(45);

#[instrument(name = "simulate", skip_all, fields(
    chain_id = %args.chain_id,
    sender = %args.sender_address.0.key().to_fixed_hex_string(),
    block_number = ?args.block_number.map(|b| b.0),
    tx_version = %args.transaction_version.0,
))]
pub async fn simulate(
    db_pool: &Pool<Postgres>,
    s3_client: &aws_sdk_s3::Client,
    execution_result: Option<ExecutionResult>,
    args: SimulationArgs,
    voyager_client: Option<&VoyagerClient>,
    external_cache: Option<&ExternalClassCache>,
    background_retry: Option<&BackgroundRetryService>,
) -> Result<SimulationOutput, TransactionSimulationError> {
    let provider_client = create_rpc_client_from_url(args.rpc_url.clone());
    let chain_id = args.chain_id.clone();
    let block_number = if let Some(bn) = args.block_number {
        bn.0
    } else {
        provider_client
            .block_number()
            .await
            .map_err(TransactionSimulationError::ProviderError)?
    };

    let (block_info, transaction_index, total_txs_in_block) =
        extract_block_txs_info(&provider_client, &args, block_number).await?;

    let block_timestamp = block_info.block_timestamp;
    let mut cached_fork_state_non_inlined_class = CachedState::new(
        ForkStateReader::new(
            args.rpc_url.clone(),
            block_number,
            transaction_index,
            total_txs_in_block,
            true,
            db_pool,
            s3_client,
            None,
        )
        .map_err(|e| {
            TransactionSimulationError::StateError(StateError::StateReadError(e.to_string()))
        })?,
    );

    let (cheatnet_state, post_exec_state_data) =
        run_simulation(block_info, args, &mut cached_fork_state_non_inlined_class)?;

    info!("Simuation done..");
    let mut contract_flamechart: Vec<FlameChartNode> = Vec::new();
    let ContractCallsMapBuilder {
        mut contract_calls_map,
        mut next_call_id,
        mut deepest_failed_contract_call_id,
        cheatnet_state_detected_events,
        ..
    } = ContractCallsMapBuilder::new_from_cheatnet_state(cheatnet_state, &mut contract_flamechart);

    debug!("Fetching classes data");
    let class_hashes = contract_calls_map.collect_all_class_hashes();
    let mut classes_data = fetch_classes_data(db_pool, s3_client, &class_hashes).await;

    debug!(
        "Walnut DB classes_data: {} entries, class_hashes: {:?}",
        classes_data.len(),
        classes_data.keys().collect::<Vec<_>>()
    );

    // Check Voyager for classes not verified on Walnut
    // This triggers synchronous compilation so function calls are available on simple(non inline) trace
    let already_verified: HashSet<String> = classes_data.keys().cloned().collect();
    let voyager_verified = check_voyager_verified_classes(
        db_pool,
        s3_client,
        voyager_client,
        &class_hashes,
        &already_verified,
        external_cache,
        background_retry,
    )
    .await;

    populate_classes_data_from_voyager_cache(
        &contract_calls_map,
        &mut classes_data,
        external_cache,
    )
    .await;

    let mut deepest_function_call_id_with_panic: Option<u32> = None;

    let (mut function_calls_map, event_calls_map, incomplete_storage_changes) =
        create_function_calls_map_generic(
            &mut contract_calls_map,
            &mut next_call_id,
            &mut deepest_function_call_id_with_panic,
            &classes_data,
            &cached_fork_state_non_inlined_class,
            true,
            build_simple_contract_call_debugger_data_adapter,
        )?;

    // Mark Voyager-verified classes for debug button.
    // Must be done AFTER create_function_calls_map_generic, which overwrites
    // call_debugger_data_available based on inline_strategy_class_hash presence.
    // TODO: Refactor logic
    if !voyager_verified.is_empty() {
        for call in contract_calls_map.0.values_mut() {
            if let Some(class_hash) = &call.class_hash {
                if voyager_verified.contains(class_hash) {
                    call.call_debugger_data_available = true;
                }
            }
        }
    }

    // Set the deepest function call that panic to true
    if let Some(deepest_func_call_id) = deepest_function_call_id_with_panic {
        if let Some(deepest_func_call) = function_calls_map.0.get(&deepest_func_call_id) {
            let contract_call_id = deepest_func_call.contract_call_id;

            let current_nested = contract_calls_map
                .0
                .get(&contract_call_id)
                .map(|c| c.contract_calls_nesting_level);

            let prev_nested = deepest_failed_contract_call_id.and_then(|id| {
                contract_calls_map
                    .0
                    .get(&id)
                    .map(|c| c.contract_calls_nesting_level)
            });

            match (current_nested, prev_nested) {
                (Some(curr), Some(prev)) => {
                    if curr >= prev {
                        deepest_failed_contract_call_id = Some(contract_call_id);
                        if let Some(call) = function_calls_map.0.get_mut(&deepest_func_call_id) {
                            call.is_deepest_panic_result = true;
                        }
                    } else if let Some(call) = function_calls_map.0.get_mut(&deepest_func_call_id) {
                        call.is_deepest_panic_result = false;
                    }
                }
                (Some(_), None) => {
                    deepest_failed_contract_call_id = Some(contract_call_id);
                }
                _ => {
                    if let Some(call) = function_calls_map.0.get_mut(&deepest_func_call_id) {
                        call.is_deepest_panic_result = false;
                    }
                }
            }
        }
    }

    let storage_changes = fetch_before_storage_changes(
        incomplete_storage_changes.unwrap_or_default(),
        &contract_calls_map,
        &provider_client,
        block_number,
    )
    .await;

    filter_and_hide_unlinked_function_calls(&mut contract_calls_map, &function_calls_map);

    let mut event_abis: Vec<Event> = Vec::new();
    let mut struct_abis: Vec<Struct> = Vec::new();
    let mut enum_abis: Vec<Enum> = Vec::new();

    for call in contract_calls_map.0.values_mut() {
        if let Some(class_hash) = call.entry_point.class_hash {
            let contract_class = cached_fork_state_non_inlined_class
                .state
                .in_memory_fork_cache
                .borrow()
                .get_contract_class(class_hash)
                .ok();
            if let Some(ContractClass::Sierra(class)) = contract_class {
                let mut abi_processor = AbiProcessor::new(call.entry_point.entry_point_selector);
                abi_processor.process_abi(&class.abi);
                call.entry_point_name = abi_processor.entry_point_function_name;
                call.entry_point_interface_name = abi_processor.entry_point_interface_name;
                call.is_erc20_token = abi_processor.is_erc20_token;
                call.arguments_names = abi_processor.function_arguments_names;
                call.arguments_types = abi_processor.function_arguments_types;
                call.result_types = abi_processor.function_return_result_types;
                let (sierra_version, cairo_version) =
                    extract_sierra_and_cairo_versions(&class.sierra_program);
                call.sierra_version = sierra_version;
                call.cairo_version = cairo_version;
                call.decode_call_result(&abi_processor.struct_abis, &abi_processor.enum_abis);
                call.decode_call_arguments(&abi_processor.struct_abis, &abi_processor.enum_abis);

                event_abis.extend(abi_processor.event_abis);
                struct_abis.extend(abi_processor.struct_abis);
                enum_abis.extend(abi_processor.enum_abis);
            }
        }
    }

    let mut contract_names_fetcher = ContractNamesFetcher::new(provider_client, &chain_id);
    //    // The StarkNet transaction emits this `Transfer` event from the StarkGate ETH token contract.
    //    // This event is present inside the transaction receipt but is not found in the Foundry-emitted
    //    // events array.
    //    // To maintain consistency with blockchain explorers, we need to manually append this event
    //    // to the vector of all events.
    //    let strkgate_emitted_event = if let Some(event) = strkgate_event {
    //        let contract_address_felt = event.from_address;
    //        let contract_address_str = contract_address_felt.to_fixed_hex_string();
    //
    //        let contract_name = contract_names_fetcher
    //            .fetch_single_contract_name(contract_address_str)
    //            .await;
    //        EmittedEvent::convert_event_to_emitted_event(&event, &contract_name)
    //    } else {
    //        None
    //    };

    let events = EmittedEvent::create_emitted_events_list(
        &mut contract_calls_map,
        &event_abis,
        &struct_abis,
        &enum_abis,
        &cheatnet_state_detected_events,
        None,
    );

    contract_names_fetcher
        .set_contract_names(&mut contract_calls_map)
        .await;

    let execution_result = get_execution_result(
        &contract_calls_map.0,
        deepest_failed_contract_call_id,
        post_exec_state_data
            .as_ref()
            .map(|post_exec_state_data| &post_exec_state_data.post_exec_report),
        execution_result,
    )?;

    let l2_flamechart = post_exec_state_data
        .as_ref()
        .and_then(|post_exec_state_data| {
            build_flamegraph(
                &post_exec_state_data.detailed_receipt,
                &contract_calls_map,
                &mut contract_flamechart,
            )
        });

    let l1_data_flamechart = post_exec_state_data
        .as_ref()
        .and_then(|post_exec_state_data| {
            build_l1_data_flamegraph(post_exec_state_data, &post_exec_state_data.detailed_receipt)
        });

    if let ExecutionResult::Reverted { reason, .. } = &execution_result {
        if let Some(call) = contract_calls_map
            .0
            .get_mut(&deepest_failed_contract_call_id.unwrap_or(1))
        {
            call.error_message = Some(reason.clone());
            call.is_deepest_panic_result = true;
        }
    }

    let execution_resources = post_exec_state_data.as_ref().map(|psd| ExecutionResources {
        l1_gas: psd.detailed_receipt.gas.l1_gas.0,
        l1_data_gas: psd.detailed_receipt.gas.l1_data_gas.0,
        l2_gas: psd.detailed_receipt.gas.l2_gas.0,
    });

    let simulation_info = SimulationInfo {
        contract_calls_map,
        function_calls_map,
        event_calls_map: event_calls_map.unwrap_or_default(),
        events,
        execution_result,
        simulation_debugger_data: None,
        storage_changes,
        estimated_fee: post_exec_state_data
            .as_ref()
            .and_then(|post_exec_state_data| {
                post_exec_state_data.detailed_receipt.estimated_fee.clone()
            }),
    };

    Ok((
        simulation_info,
        block_timestamp,
        transaction_index,
        total_txs_in_block,
        l2_flamechart,
        l1_data_flamechart,
        execution_resources,
    ))
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

/// Wait for Phase 1 (non-inline build) to complete, then populate `classes_data`
/// with the original contract class (matching on-chain CASM) from the cache.
///
/// Waits at most 30 seconds for Phase 1; logs an error if it times out.
/// Phase 2 (inline build) runs in the background and is not awaited here.
async fn populate_classes_data_from_voyager_cache(
    contract_calls_map: &ContractCallsMap,
    classes_data: &mut HashMap<String, DataWithContractClass>,
    external_cache: Option<&ExternalClassCache>,
) {
    let Some(cache) = external_cache else {
        return;
    };

    let missing_class_hashes: Vec<String> = {
        let mut seen = HashSet::new();
        contract_calls_map
            .0
            .values()
            .filter_map(|call| call.class_hash.as_ref())
            .filter(|h| !classes_data.contains_key(*h) && seen.insert((*h).clone()))
            .cloned()
            .collect()
    };

    debug!(
        "Missing class hashes (not in Walnut DB): {:?}",
        missing_class_hashes
    );

    // Wait for Phase 1 (non-inline) to complete for all classes in parallel.
    // If compilation doesn't finish in time, return without function calls.
    let phase1_waits: Vec<_> = missing_class_hashes
        .iter()
        .map(|class_hash_str| {
            let cache = cache.clone();
            let class_hash_str = class_hash_str.clone();
            async move {
                if cache.is_phase1_pending(&class_hash_str).await {
                    match tokio::time::timeout(
                        VOYAGER_PHASE_1_TIMEOUT,
                        cache.wait_for_phase1_ready(&class_hash_str),
                    )
                    .await
                    {
                        Ok(_) => {}
                        Err(_) => {
                            warn!(
                                "Timeout ({:?}) waiting for Phase 1 compilation of {} — function calls are unavailable for this simulation",
                                VOYAGER_PHASE_1_TIMEOUT, class_hash_str
                            );
                        }
                    }
                }
            }
        })
        .collect();
    future::join_all(phase1_waits).await;

    for class_hash_str in missing_class_hashes {
        if let Some(cached) = cache.get(&class_hash_str).await {
            info!(
                "Cache HIT for {}: has original_contract_class={}, has inline_data={}, source={}",
                class_hash_str,
                cached.original_contract_class.is_some(),
                cached.data.is_some(),
                cached.source
            );
            if let Some(original_class) = &cached.original_contract_class {
                classes_data.insert(
                    class_hash_str.clone(),
                    DataWithContractClass {
                        inline_strategy_class_hash: cached.inline_strategy_class_hash.clone(),
                        contract_class: original_class.clone(),
                    },
                );
                info!(
                    "Added Voyager original class {} to classes_data",
                    class_hash_str
                );
            } else {
                info!(
                    "No original_contract_class for {}, skipping",
                    class_hash_str
                );
            }
        } else {
            debug!(
                "Cache miss for {} after waiting for Phase 1",
                class_hash_str
            );
        }
    }

    debug!(
        "classes_data after Voyager cache: {} entries",
        classes_data.len()
    );
}

#[instrument(name = "run_simulation", skip_all, fields(
    sender = %args.sender_address.0.key().to_fixed_hex_string(),
    tx_type = ?args.transaction_type,
    calldata_len = args.calldata.0.len(),
))]
fn run_simulation(
    block_info: BlockInfo,
    args: SimulationArgs,
    cached_fork_state_non_inlined_class: &mut CachedState<ForkStateReader>,
) -> Result<(CheatnetState, Option<PostExecStateData>), TransactionSimulationError> {
    debug!("Running simulation...");
    let signature_len = args.transaction_signature.as_ref().map_or(0, |s| s.0.len());
    let calldata_len = args.calldata.0.len();
    let transaction_context = extract_transaction_contex(&args, &block_info);

    let mut cheatnet_state_non_inlined = CheatnetState {
        block_info,
        ..Default::default()
    };

    cheatnet_state_non_inlined.trace_data.is_vm_trace_needed = true;

    let initial_gas = transaction_context.initial_sierra_gas();
    let mut initial_gas_counter = GasCounter::new(initial_gas);

    // Extract block number for error conversion
    let block_number = cached_fork_state_non_inlined_class.state.block_number();

    let (validate, execute) = execute_transaction_flows_with_executor(
        &args,
        cached_fork_state_non_inlined_class,
        &mut cheatnet_state_non_inlined,
        &mut initial_gas_counter,
        transaction_context.clone(),
        &|call, state, cheatnet_state, ctx, _| {
            let mut remaining_gas = call.initial_gas;
            execute_call_entry_point(
                call,
                state,
                cheatnet_state,
                ctx,
                &mut remaining_gas,
                &ExecuteCallEntryPointExtraOptions {
                    trace_data_handled_by_revert_call: false,
                },
            )
            .map_err(|err: AnnotatedEntryPointExecutionError| {
                convert_entry_point_error_with_block(err.into_unannotated(), block_number)
            })
        },
        &|call, state, cheatnet_state, ctx, _| {
            let mut remaining_gas = call.initial_gas;
            execute_call_entry_point(
                call,
                state,
                cheatnet_state,
                ctx,
                &mut remaining_gas,
                &ExecuteCallEntryPointExtraOptions {
                    trace_data_handled_by_revert_call: false,
                },
            )
            .map_err(|err: AnnotatedEntryPointExecutionError| {
                convert_entry_point_error_with_block(err.into_unannotated(), block_number)
            })
        },
    )?;

    let post_exec_state_data = if args.transaction_version == TransactionVersion::THREE {
        let post_exec_state_data = handle_post_exec_and_collect_gas_vectors(
            &args,
            cached_fork_state_non_inlined_class,
            &mut cheatnet_state_non_inlined,
            transaction_context.clone(),
            &validate,
            &execute,
            signature_len,
            calldata_len,
        )?;
        Some(post_exec_state_data)
    } else {
        None
    };

    Ok((cheatnet_state_non_inlined, post_exec_state_data))
}

#[instrument(name = "simulate_by_calldata", skip_all, fields(
    chain_id = %args.chain_id,
    sender = %args.sender_address.0.key().to_fixed_hex_string(),
    block_number = ?args.block_number.map(|b| b.0),
))]
pub async fn simulate_by_calldata(
    db_pool: &Pool<Postgres>,
    s3_client: &aws_sdk_s3::Client,
    args: SimulationArgs,
    voyager_client: Option<&VoyagerClient>,
    external_cache: Option<&ExternalClassCache>,
    background_retry: Option<&BackgroundRetryService>,
) -> Result<TransactionSimulationResult, TransactionSimulationError> {
    let nonce: Option<u64> = match args.nonce {
        Some(nonce) => nonce.0.to_u64(),
        None => None,
    };
    let readable_chain_id = chain_id_to_readable_string(&args.chain_id);

    // Store original block_number info before args is moved
    let latest_block = args.block_number.is_none();

    // Resolve block number if it's None (Latest) - same logic as in simulate function
    let resolved_block_number = if let Some(bn) = args.block_number {
        bn.0
    } else {
        let provider_client = create_rpc_client_from_url(args.rpc_url.clone());
        provider_client
            .block_number()
            .await
            .map_err(TransactionSimulationError::ProviderError)?
    };

    let block_number = BlockId::Number(resolved_block_number);

    let sender_address = args.sender_address;
    let calldata = args
        .calldata
        .0
        .to_vec()
        .iter()
        .map(|felt| felt.to_hex_string())
        .collect::<Vec<String>>();
    let transaction_version: usize = args.transaction_version.0.to_u64().unwrap() as usize;

    let walnut_webapp_url = Some(walnut_shared::simulation_deep_link(
        &sender_address.to_string(),
        &calldata,
        transaction_version,
        &args.chain_id,
        args.block_number.map(|b| b.0),
        args.rpc_url.as_str(),
    ));

    let (
        simulation_result,
        block_timestamp,
        transaction_index_in_block,
        total_transactions_in_block,
        l2_flamechart,
        l1_data_flamechart,
        execution_resources,
    ) = simulate(
        db_pool,
        s3_client,
        None,
        args,
        voyager_client,
        external_cache,
        background_retry,
    )
    .await?;

    let l2_transaction_data = L2TransactionData {
        simulation_result,
        chain_id: readable_chain_id,
        block_number,
        block_timestamp: block_timestamp.0,
        nonce,
        sender_address,
        calldata,
        transaction_version,
        transaction_type: transaction_type_to_string(TransactionType::InvokeFunction),
        transaction_index_in_block: Some(transaction_index_in_block),
        total_transactions_in_block: Some(total_transactions_in_block),
        l1_tx_hash: None,
        l2_tx_hash: None,
        flamechart: l2_flamechart,
        l1_data_flamechart,
        actual_fee: None,
        execution_resources,
        latest_block,
    };
    Ok(TransactionSimulationResult {
        l1_transaction_data: None,
        l2_transaction_data: Some(l2_transaction_data),
        walnut_webapp_url,
    })
}

#[instrument(name = "simulate_by_tx_hash", skip_all, fields(
    tx_hash,
    network = %network,
))]
pub async fn simulate_transaction_by_hash(
    db_pool: &Pool<Postgres>,
    s3_client: &aws_sdk_s3::Client,
    starknet_rpc_url: Option<Url>,
    ethereum_rpc_url: Option<String>,
    tx_hash: &str,
    chain_id: Option<EChainId>,
    network: &ENetwork,
    voyager_client: Option<&VoyagerClient>,
    external_cache: Option<&ExternalClassCache>,
    background_retry: Option<&BackgroundRetryService>,
) -> Result<TransactionSimulationResult, TransactionSimulationError> {
    if let Some(transaction_hash) = parse_transaction_hash_per_network(tx_hash, network) {
        let rpc_url_str = starknet_rpc_url.as_ref().map(|u| u.as_str().to_string());
        let mut result = match transaction_hash {
            ETransactionHashType::Starknet(starknet_hash) => {
                simulate_starknet_transaction_by_hash(
                    db_pool,
                    s3_client,
                    starknet_rpc_url,
                    ethereum_rpc_url,
                    starknet_hash,
                    chain_id,
                    voyager_client,
                    external_cache,
                    background_retry,
                )
                .await?
            }
            ETransactionHashType::Ethereum(ethereum_hash) => {
                simulate_ethereum_transaction_by_hash(
                    db_pool,
                    s3_client,
                    starknet_rpc_url,
                    ethereum_rpc_url,
                    ethereum_hash,
                    chain_id,
                    voyager_client,
                    external_cache,
                    background_retry,
                )
                .await?
            }
        };
        let chain_id_url = chain_id.map(|c| chain_id_to_url_format(&ChainId::from(c)));
        result.walnut_webapp_url = Some(walnut_shared::transaction_deep_link(
            chain_id_url.as_deref(),
            rpc_url_str.as_deref(),
            tx_hash,
        ));
        Ok(result)
    } else {
        Err(TransactionSimulationError::TransactionHashNotFound(
            tx_hash.to_string(),
            network.to_string(),
        ))
    }
}

#[instrument(name = "simulate_starknet_tx", skip_all, fields(
    tx_hash = %transaction_hash.to_hex_string(),
    chain_id = ?chain_id,
))]
async fn simulate_starknet_transaction_by_hash(
    db_pool: &Pool<Postgres>,
    s3_client: &aws_sdk_s3::Client,
    starknet_rpc_url: Option<Url>,
    _ethereum_rpc_url: Option<String>,
    transaction_hash: Felt,
    chain_id: Option<EChainId>,
    voyager_client: Option<&VoyagerClient>,
    external_cache: Option<&ExternalClassCache>,
    background_retry: Option<&BackgroundRetryService>,
) -> Result<TransactionSimulationResult, TransactionSimulationError> {
    let starknet_rpc_url = starknet_rpc_url.ok_or(TransactionSimulationError::InvalidRpcUrl)?;
    let provider_client = create_rpc_client_from_url(starknet_rpc_url.clone());
    // Fetch transaction
    let transaction = provider_client
        .get_transaction_by_hash(transaction_hash, None)
        .await;
    if let Ok(transaction) = transaction {
        // Check if it's a DEPLOY or DEPLOY_ACCOUNT transaction (not supported)
        if matches!(transaction, Transaction::Deploy(_)) {
            let tx_hash_str = transaction_hash.to_hex_string();
            return Err(TransactionSimulationError::DeployTransactionNotSupported(
                "DEPLOY".to_string(),
                tx_hash_str,
            ));
        }
        if matches!(transaction, Transaction::DeployAccount(_)) {
            let tx_hash_str = transaction_hash.to_hex_string();
            return Err(TransactionSimulationError::DeployTransactionNotSupported(
                "DEPLOY_ACCOUNT".to_string(),
                tx_hash_str,
            ));
        }

        if let Some((
            nonce,
            sender_address,
            entry_point_selector,
            calldata,
            transaction_version,
            transaction_type,
            signature,
            max_fee,
            resource_bounds,
            paymaster_data,
        )) = extract_submitted_tx(transaction)
        {
            // Fetch receipt
            let transaction_receipt_with_block_info = provider_client
                .get_transaction_receipt(transaction_hash)
                .await;
            if let Ok(transaction_receipt_with_block_info) = transaction_receipt_with_block_info {
                let transaction_receipt = transaction_receipt_with_block_info.receipt;
                let block_info = transaction_receipt_with_block_info.block;

                match block_info {
                    ReceiptBlock::PreConfirmed { .. } => {
                        return Err(TransactionSimulationError::PreConfirmedBlock(
                            "Transaction simualtion in pre-confirmed block not found".to_string(),
                        ));
                    }
                    ReceiptBlock::Block { block_number, .. } => {
                        // Fetch or derive chain_id
                        let chain_id = match chain_id {
                            Some(chain_id) => ChainId::from(chain_id),
                            None => extract_chain_id_from_felt(
                                provider_client.chain_id().await.map_err(|_| {
                                    TransactionSimulationError::FailedToFetchChainId
                                })?,
                            )?,
                        };

                        let tx_execution_result =
                            extract_execution_status_transaction_receipt(&transaction_receipt);
                        // Check for execution status
                        if let Some(ExecutionResult::Reverted { ref reason }) = tx_execution_result
                        {
                            if reason.contains("RunResources") {
                                // Fetch block timestamp
                                let block_timestamp =
                                    extract_block_timestamp(&provider_client, block_number).await?;

                                let simulation_info = SimulationInfo {
                                    contract_calls_map: ContractCallsMap::new(),
                                    function_calls_map: FunctionCallsMap::new(),
                                    event_calls_map: EventCallsMap::default(),
                                    events: Vec::new(),
                                    execution_result: ExecutionResult::Reverted {
                                        reason: reason.to_string(),
                                    },
                                    simulation_debugger_data: Some(SimulationDebuggerData {
                                        classes_debugger_data: HashMap::new(),
                                        debugger_trace: Vec::new(),
                                        initial_step_index: None,
                                    }),
                                    storage_changes: HashMap::new(),
                                    estimated_fee: None,
                                };
                                let l2_transaction_data = L2TransactionData {
                                    simulation_result: simulation_info,
                                    chain_id: chain_id_to_readable_string(&chain_id),
                                    block_number: BlockId::Number(block_number),
                                    block_timestamp: block_timestamp.0,
                                    nonce: nonce.0.to_u64(),
                                    sender_address,
                                    calldata: calldata_to_hex(&calldata),
                                    transaction_version: transaction_version.0.to_u64().unwrap()
                                        as usize,
                                    transaction_type: transaction_type_to_string(transaction_type),
                                    transaction_index_in_block: None,
                                    total_transactions_in_block: None,
                                    l1_tx_hash: None,
                                    l2_tx_hash: Some(transaction_hash.to_hex_string()),
                                    flamechart: None,
                                    l1_data_flamechart: None,
                                    actual_fee: None,
                                    execution_resources: None,
                                    latest_block: false, // This is from tx hash, so not Latest
                                };
                                return Ok(TransactionSimulationResult {
                                    l1_transaction_data: None,
                                    l2_transaction_data: Some(l2_transaction_data),
                                    walnut_webapp_url: None,
                                });
                            }
                        }

                        let strkgate_event =
                            extract_starkgate_event_transaction_receipt(&transaction_receipt);
                        let actual_fee =
                            extract_actual_fee_transaction_receipt(&transaction_receipt);
                        let execution_resources =
                            extract_execution_resources_transaction_receipt(&transaction_receipt);
                        // Perform transaction simulation
                        let (
                            simulation_result,
                            block_timestamp,
                            transaction_index_in_block,
                            total_transactions_in_block,
                            l2_flamechart,
                            l1_data_flamechart,
                            _,
                        ) = simulate(
                            db_pool,
                            s3_client,
                            tx_execution_result,
                            SimulationArgs {
                                rpc_url: starknet_rpc_url.clone(),
                                chain_id: chain_id.clone(),
                                block_number: Some(BlockNumber(block_number)),
                                nonce: Some(nonce),
                                sender_address,
                                entry_point_selector: Some(entry_point_selector),
                                calldata: calldata.clone(),
                                transaction_version,
                                transaction_signature: Some(signature),
                                transaction_hash: Some(TransactionHash(transaction_hash)),
                                transaction_type: Some(transaction_type),
                                max_fee: Some(max_fee),
                                resource_bounds: Some(resource_bounds),
                                paymaster_data: Some(paymaster_data),
                                strkgate_event,
                            },
                            voyager_client,
                            external_cache,
                            background_retry,
                        )
                        .await?;
                        // Build and return simulation result
                        let l2_transaction_data = L2TransactionData {
                            simulation_result,
                            chain_id: chain_id_to_readable_string(&chain_id),
                            block_number: BlockId::Number(block_number),
                            block_timestamp: block_timestamp.0,
                            nonce: nonce.0.to_u64(),
                            sender_address,
                            calldata: calldata_to_hex(&calldata),
                            transaction_version: transaction_version.0.to_u64().unwrap() as usize,
                            transaction_type: transaction_type_to_string(transaction_type),
                            transaction_index_in_block: Some(transaction_index_in_block),
                            total_transactions_in_block: Some(total_transactions_in_block),
                            l1_tx_hash: None,
                            l2_tx_hash: Some(transaction_hash.to_hex_string()),
                            flamechart: l2_flamechart,
                            l1_data_flamechart,
                            actual_fee,
                            execution_resources,
                            latest_block: false, // This is from tx hash, so not Latest
                        };
                        return Ok(TransactionSimulationResult {
                            l1_transaction_data: None,
                            l2_transaction_data: Some(l2_transaction_data),
                            walnut_webapp_url: None,
                        });
                    }
                }
            }
        }
    }
    let tx_hash_str = transaction_hash.to_hex_string();
    let network = if let Some(chain_id) = chain_id {
        let chain_id = ChainId::from(chain_id);
        chain_id_to_readable_string(&chain_id)
    } else {
        mask_private_token_in_url(&starknet_rpc_url)
    };
    Err(TransactionSimulationError::TransactionHashNotFound(
        tx_hash_str,
        network,
    ))
}

#[instrument(name = "simulate_eth_tx", skip_all, fields(
    tx_hash = %transaction_hash,
    chain_id = ?chain_id,
))]
pub async fn simulate_ethereum_transaction_by_hash(
    db_pool: &Pool<Postgres>,
    s3_client: &aws_sdk_s3::Client,
    starknet_rpc_url: Option<Url>,
    ethereum_rpc_url: Option<String>,
    transaction_hash: H256,
    chain_id: Option<EChainId>,
    voyager_client: Option<&VoyagerClient>,
    external_cache: Option<&ExternalClassCache>,
    background_retry: Option<&BackgroundRetryService>,
) -> Result<TransactionSimulationResult, TransactionSimulationError> {
    let chain_id = chain_id.ok_or(TransactionSimulationError::InvalidChainId)?;
    let ethereum_rpc_url = ethereum_rpc_url.ok_or(TransactionSimulationError::InvalidRpcUrl)?;
    let starknet_rpc_url = starknet_rpc_url.ok_or(TransactionSimulationError::InvalidRpcUrl)?;

    // Fetch the tx from L1HandlerTransaction
    let provider_eth_client = create_eth_provider_from_url(ethereum_rpc_url.clone());
    let transaction_receipt = provider_eth_client
        .get_transaction_receipt(transaction_hash)
        .await
        .map_err(|_| {
            TransactionSimulationError::TransactionHashNotFound(
                transaction_hash.to_string(),
                mask_private_token_in_url(&ethereum_rpc_url),
            )
        })?
        .ok_or(TransactionSimulationError::TransactionHashNotFound(
            transaction_hash.to_string(),
            mask_private_token_in_url(&ethereum_rpc_url),
        ))?;

    // Process logs - collect all L1 and L2 events
    let mut l1_l2_events = Vec::new();
    let mut l2_l1_messages = Vec::new();

    for log in &transaction_receipt.logs {
        if let Some(l1_l2_event) = decode_l1_l2_event(log) {
            l1_l2_events.push(l1_l2_event);
        } else if let Some(l2msg) = decode_l2_l1_event(log) {
            l2_l1_messages.push(l2msg);
        }
    }

    // Handle L1 handler transactions
    if let Some(l1_l2_event) = l1_l2_events.first() {
        return process_l1_handler_transaction(
            db_pool,
            s3_client,
            &starknet_rpc_url,
            chain_id,
            transaction_hash,
            l1_l2_event.clone(),
            voyager_client,
            external_cache,
            background_retry,
        )
        .await;
    }

    process_l1_transaction_data(
        chain_id,
        transaction_hash,
        &transaction_receipt,
        l2_l1_messages,
    )
}

async fn process_l1_handler_transaction(
    db_pool: &Pool<Postgres>,
    s3_client: &aws_sdk_s3::Client,
    starknet_rpc_url: &Url,
    chain_id: EChainId,
    transaction_hash: H256,
    l1_event: EStarknetL1L2Event,
    voyager_client: Option<&VoyagerClient>,
    external_cache: Option<&ExternalClassCache>,
    background_retry: Option<&BackgroundRetryService>,
) -> Result<TransactionSimulationResult, TransactionSimulationError> {
    let l1_handler_tx = L1HandlerTransaction::try_from(l1_event)
        .map_err(|_| TransactionSimulationError::ConversionError)?;

    let l2_chain_id = to_chain_id(&chain_id);
    let core_l2_chain_id = ChainId::from(l2_chain_id);

    let l2_tx_hash = l1_handler_tx
        .calculate_transaction_hash(
            &core_l2_chain_id,
            &starknet_api::transaction::TransactionVersion::ZERO,
        )
        .ok();

    let l2_tx_hash_hex = l2_tx_hash.as_ref().map(|hash| hash.to_hex_string());
    let block_number = match &l2_tx_hash_hex {
        Some(hash) => fetch_tx_block_number_from_voyager(&core_l2_chain_id, hash)
            .await
            .ok(),
        None => None,
    };

    let (
        simulation_result,
        block_timestamp,
        transaction_index_in_block,
        total_transactions_in_block,
        _,
        _,
        _,
    ) = simulate(
        db_pool,
        s3_client,
        None,
        SimulationArgs {
            rpc_url: starknet_rpc_url.clone(),
            chain_id: core_l2_chain_id.clone(),
            block_number: block_number.map(BlockNumber),
            nonce: Some(l1_handler_tx.nonce),
            sender_address: l1_handler_tx.contract_address,
            entry_point_selector: Some(l1_handler_tx.entry_point_selector),
            calldata: l1_handler_tx.calldata.clone(),
            transaction_version: TransactionVersion::ZERO,
            transaction_signature: None,
            transaction_hash: l2_tx_hash,
            transaction_type: Some(TransactionType::L1Handler),
            max_fee: None,
            resource_bounds: None,
            paymaster_data: None,
            strkgate_event: None,
        },
        voyager_client,
        external_cache,
        background_retry,
    )
    .await?;

    let l2_transaction_data = L2TransactionData {
        simulation_result,
        chain_id: chain_id_to_readable_string(&core_l2_chain_id),
        block_number: block_number.map_or_else(
            || {
                starknet_rust::core::types::BlockId::Tag(
                    starknet_rust::core::types::BlockTag::Latest,
                )
            },
            BlockId::Number,
        ),
        block_timestamp: block_timestamp.0,
        nonce: l1_handler_tx.nonce.0.to_u64(),
        sender_address: l1_handler_tx.contract_address,
        calldata: calldata_to_hex(&l1_handler_tx.calldata),
        transaction_version: TransactionVersion::ZERO.0.to_u64().unwrap() as usize,
        transaction_type: transaction_type_to_string(TransactionType::L1Handler),
        transaction_index_in_block: Some(transaction_index_in_block),
        total_transactions_in_block: Some(total_transactions_in_block),
        l1_tx_hash: Some(transaction_hash.encode_hex()),
        l2_tx_hash: l2_tx_hash_hex,
        flamechart: None,
        l1_data_flamechart: None,
        actual_fee: None,
        execution_resources: None,
        latest_block: block_number.is_none(), // This is L1 handler, check if block_number was None
    };

    Ok(TransactionSimulationResult {
        l1_transaction_data: None,
        l2_transaction_data: Some(l2_transaction_data),
        walnut_webapp_url: None,
    })
}

fn process_l1_transaction_data(
    chain_id: EChainId,
    transaction_hash: H256,
    receipt: &EthTransactionReceipt,
    messages: Vec<DecodedL2ToL1Message>,
) -> Result<TransactionSimulationResult, TransactionSimulationError> {
    let core_l1_chain_id = ChainId::from(chain_id);

    let transaction_type = receipt.transaction_type.and_then(|t| match t.as_u64() {
        0 => Some("Legacy".to_string()),
        1 => Some("EIP-2930".to_string()),
        2 => Some("EIP-1559".to_string()),
        3 => Some("EIP-4844".to_string()),
        _ => None,
    });

    let status = receipt.status.and_then(|s| match s.as_u64() {
        1 => Some("SUCCEEDED".to_string()),
        0 => Some("REVERTED".to_string()),
        _ => None,
    });

    let message_hashes: Vec<String> = messages
        .iter()
        .map(|msg| msg.message_hash.clone())
        .collect();

    let l1_transaction_data = L1TransactionData {
        chain_id: chain_id_to_readable_string(&core_l1_chain_id),
        block_number: receipt.block_number.map(|bn| bn.as_u64()),
        sender_address: format!("{:#x}", receipt.from),
        receiver_address: receipt.to.map(|addr| format!("{:#x}", addr)),
        transaction_type,
        status,
        message_hashes,
        l1_tx_hash: Some(transaction_hash.encode_hex()),
    };

    Ok(TransactionSimulationResult {
        l1_transaction_data: Some(l1_transaction_data),
        l2_transaction_data: None,
        walnut_webapp_url: None,
    })
}

fn decode_l1_l2_event(log: &Log) -> Option<EStarknetL1L2Event> {
    let topics = log.topics.clone();
    if topics.is_empty() {
        return None;
    }

    let event_signature = topics[0];
    // TODO: Add other events
    match event_signature {
        sig if sig
            == H256::from(keccak256(
                "LogMessageToL2(address,uint256,uint256,uint256[],uint256,uint256)",
            )) =>
        {
            if topics.len() < 4 {
                return None;
            }

            let from_address = Address::from(topics[1]);
            let to_address = U256::from_big_endian(topics[2].as_fixed_bytes());
            let selector = U256::from_big_endian(topics[3].as_fixed_bytes());

            <(Vec<U256>, U256, U256)>::decode(&log.data)
                .ok()
                .map(|(payload, nonce, fee)| EStarknetL1L2Event::LogMessageToL2 {
                    from_address,
                    to_address,
                    selector,
                    payload,
                    nonce,
                    fee,
                })
        }

        _ => None,
    }
}

fn decode_l2_l1_event(log: &Log) -> Option<DecodedL2ToL1Message> {
    let topics = &log.topics;
    if topics.is_empty() {
        return None;
    }

    let event_signature = topics[0];

    match event_signature {
        sig if sig == H256::from(keccak256("ConsumedMessageToL1(uint256,address,uint256[])")) => {
            if topics.len() < 3 {
                return None;
            }

            let from_address = U256::from_big_endian(topics[1].as_fixed_bytes());
            let to_address = H160::from_slice(&topics[2].as_bytes()[12..]);

            let payload: Vec<U256> = <Vec<U256>>::decode(&log.data).ok()?;

            let mut bytes = Vec::new();

            let mut from_bytes = [0u8; 32];
            from_address.to_big_endian(&mut from_bytes);
            bytes.extend_from_slice(&from_bytes);

            let mut to_bytes = [0u8; 32];
            to_bytes[12..].copy_from_slice(to_address.as_bytes());
            bytes.extend_from_slice(&to_bytes);

            let mut len_bytes = [0u8; 32];
            U256::from(payload.len()).to_big_endian(&mut len_bytes);
            bytes.extend_from_slice(&len_bytes);

            for p in &payload {
                let mut p_bytes = [0u8; 32];
                p.to_big_endian(&mut p_bytes);
                bytes.extend_from_slice(&p_bytes);
            }

            let hash = keccak256(&bytes);
            let message_hash = format!("0x{}", hex::encode(hash));

            Some(DecodedL2ToL1Message {
                event: EStarknetL2L1Event::ConsumedMessageToL1 {
                    from_address,
                    to_address,
                    payload,
                },
                message_hash,
            })
        }

        _ => None,
    }
}
