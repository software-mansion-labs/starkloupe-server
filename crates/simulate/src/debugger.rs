use crate::contract_calls_map::ContractCallsMap;
use crate::contract_calls_map::ContractCallsMapBuilder;
use crate::debugger_trace::DebuggerTraceBuilder;
use crate::execution::execute_transaction_flows_with_executor;
use crate::function_calls::create_function_calls_map_generic;
use crate::gas_counter::GasCounter;
use crate::state::ForkStateReader;
use crate::transaction_extraction::extract_block_txs_info;
use crate::transaction_extraction::extract_transaction_contex;
use crate::transaction_extraction::extract_transaction_signature;
use crate::DebuggerInfo;
use crate::SimulationArgs;
use crate::TransactionSimulationError;
use blockifier::state::cached_state::CachedState;
use blockifier::state::errors::StateError;
use cheatnet::runtime_extensions::call_to_blockifier_runtime_extension::execution::entry_point::{
    execute_call_entry_point, ExecuteCallEntryPointExtraOptions,
};
use cheatnet::state::CheatnetState;
use internal_tracing::build_debugger_data::build_contract_call_debugger_data_adapter;
use internal_tracing::build_debugger_data::debugger_data_maps_full_class_to_class;
use internal_tracing::debugger_data_fetcher::fetch_classes_debugger_data_with_external;
use internal_tracing::external_class_cache::ExternalClassCache;
use internal_tracing::function_calls_map::FunctionCallsMap;
use internal_tracing::SimulationDebuggerData;
use sqlx::Pool;
use sqlx::Postgres;
use starknet_api::block::BlockInfo;
use starknet_api::execution_resources::GasAmount;
use starknet_rust::core::types::Felt;
use starknet_rust::providers::Provider;
use verification::voyager::VoyagerClient;
use walnut_shared::create_rpc_client_from_url;

async fn simulate_to_get_debug_info(
    db_pool: &Pool<Postgres>,
    s3_client: &aws_sdk_s3::Client,
    args: SimulationArgs,
    external_cache: Option<&ExternalClassCache>,
    voyager_client: Option<&VoyagerClient>,
) -> Result<DebuggerInfo, TransactionSimulationError> {
    let provider_client = create_rpc_client_from_url(args.rpc_url.clone());
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

    // Cached for the debug inline class, if class is verified
    let mut cached_fork_state = CachedState::new(
        ForkStateReader::new(
            args.rpc_url.clone(),
            block_number,
            transaction_index,
            total_txs_in_block,
            false,
            db_pool,
            s3_client,
            external_cache.cloned(),
        )
        .map_err(|e| {
            TransactionSimulationError::StateError(StateError::StateReadError(e.to_string()))
        })?,
    );

    let mut args = args; // make mutable

    if let Some(transaction_hash) = args.transaction_hash {
        let transaction_str = transaction_hash.to_hex_string();
        let transaction_felt = Felt::from_hex(&transaction_str)?;
        if let Ok(transaction) = provider_client
            .get_transaction_by_hash(transaction_felt)
            .await
        {
            if let Some(signature) = extract_transaction_signature(transaction) {
                args.transaction_signature = Some(signature);
            }
        }
    }
    let cheatnet_state =
        run_simulation_to_get_debug_info(block_info, args, &mut cached_fork_state)?;

    let ContractCallsMapBuilder {
        mut contract_calls_map,
        mut next_call_id,
        ..
    } = ContractCallsMapBuilder::new_from_cheatnet_state(cheatnet_state, &mut vec![]);

    let class_hashes = contract_calls_map.collect_all_class_hashes();

    let classes_debugger_data = fetch_classes_debugger_data_with_external(
        db_pool,
        s3_client,
        &class_hashes,
        external_cache,
        voyager_client,
    )
    .await;

    let mut deepest_function_call_id_with_panic: Option<u32> = None;

    let (mut function_calls_map, _, _) = create_function_calls_map_generic(
        &mut contract_calls_map,
        &mut next_call_id,
        &mut deepest_function_call_id_with_panic,
        &classes_debugger_data,
        &cached_fork_state,
        false,
        build_contract_call_debugger_data_adapter,
    )?;

    let debugger_trace =
        DebuggerTraceBuilder::build(&1, &mut function_calls_map, &mut contract_calls_map);

    let initial_step_index = find_initial_debugger_step(&contract_calls_map, &function_calls_map);

    // Override step indices on contract/function calls to skip wrapper/unsafe boilerplate,
    // so the frontend can use them directly without duplicating the skipping logic.
    apply_real_entrypoint_step_indices(&mut contract_calls_map, &mut function_calls_map);

    let debugger_info = DebuggerInfo {
        contract_calls_map,
        function_calls_map,
        simulation_debugger_data: Some(SimulationDebuggerData {
            classes_debugger_data: debugger_data_maps_full_class_to_class(classes_debugger_data),
            debugger_trace,
            initial_step_index,
        }),
    };

    Ok(debugger_info)
}

fn run_simulation_to_get_debug_info(
    block_info: BlockInfo,
    args: SimulationArgs,
    cached_fork_state: &mut CachedState<ForkStateReader>,
) -> Result<CheatnetState, TransactionSimulationError> {
    let transaction_context = extract_transaction_contex(&args, &block_info);

    let mut cheatnet_state = CheatnetState {
        block_info: block_info.clone(),
        ..Default::default()
    };

    cheatnet_state.trace_data.is_vm_trace_needed = true;

    let (_, _) = execute_transaction_flows_with_executor(
        &args,
        cached_fork_state,
        &mut cheatnet_state,
        &mut GasCounter::new(GasAmount(u64::MAX)),
        transaction_context.clone(),
        &|call, state, cheatnet_state, ctx, _revert| {
            let mut remaining_gas = call.initial_gas;
            Ok(execute_call_entry_point(
                call,
                state,
                cheatnet_state,
                ctx,
                &mut remaining_gas,
                &ExecuteCallEntryPointExtraOptions {
                    trace_data_handled_by_revert_call: false,
                },
            )?)
        },
        &|call, state, cheatnet_state, ctx, _revert| {
            let mut remaining_gas = call.initial_gas;
            Ok(execute_call_entry_point(
                call,
                state,
                cheatnet_state,
                ctx,
                &mut remaining_gas,
                &ExecuteCallEntryPointExtraOptions {
                    trace_data_handled_by_revert_call: false,
                },
            )?)
        },
    )?;

    Ok(cheatnet_state)
}

pub async fn debug_by_calldata(
    db_pool: &Pool<Postgres>,
    s3_client: &aws_sdk_s3::Client,
    args: SimulationArgs,
    external_cache: Option<&ExternalClassCache>,
    voyager_client: Option<&VoyagerClient>,
) -> Result<DebuggerInfo, TransactionSimulationError> {
    let debugger_info =
        simulate_to_get_debug_info(db_pool, s3_client, args, external_cache, voyager_client)
            .await?;

    Ok(debugger_info)
}

/// Finds the initial debugger step index by looking for the real entrypoint function
/// inside the account contract's `__validate__` or `__execute__` call.
///
/// If the account contract is not verified (no function calls available), falls back to
/// finding the entry point of the first verified contract's real entrypoint, skipping
/// `__wrapper__` and `unsafe_` function calls.
fn find_initial_debugger_step(
    contract_calls_map: &ContractCallsMap,
    function_calls_map: &FunctionCallsMap,
) -> Option<usize> {
    // Try __validate__ first, then __execute__
    let entry_points = ["__validate__", "__execute__"];

    for entry_point_name in &entry_points {
        let account_contract_call = contract_calls_map
            .0
            .values()
            .find(|c| c.entry_point_name.as_deref() == Some(entry_point_name));

        if let Some(call) = account_contract_call {
            // If the account contract is verified, use its function call to find the real entrypoint
            if let Some(step) = find_real_entrypoint_step(call, function_calls_map) {
                return Some(step);
            }

            // Account contract is not verified — find the first verified child contract call
            if let Some(step) = find_first_verified_child_entrypoint_step(
                call,
                contract_calls_map,
                function_calls_map,
            ) {
                return Some(step);
            }
        }
    }

    None
}

/// Given a contract call with a function_call_id, finds the real entrypoint step index
/// by skipping `__wrapper__` and `unsafe_` boilerplate functions.
fn find_real_entrypoint_step(
    contract_call: &crate::contract_call::ContractCall,
    function_calls_map: &FunctionCallsMap,
) -> Option<usize> {
    let wrapper_fn_id = contract_call.function_call_id?;
    let wrapper_fn = function_calls_map.0.get(&wrapper_fn_id)?;
    find_real_step_for_function(wrapper_fn, function_calls_map)
}

/// Given a function call, finds the `debugger_trace_step_index` of the first child
/// that is not hidden, not a `__wrapper__`, and not an `unsafe_new_` function.
fn find_real_step_for_function(
    fc: &internal_tracing::function_call::FunctionCall,
    function_calls_map: &FunctionCallsMap,
) -> Option<usize> {
    fc.children_call_ids
        .iter()
        .filter_map(|child_id| function_calls_map.0.get(child_id))
        .find(|child| {
            !child.is_hidden
                && !child.fn_name.as_ref().map_or(false, |n| {
                    n.contains("unsafe_new_") || n.contains("__wrapper__")
                })
        })
        .and_then(|child| child.debugger_trace_step_index)
}

fn is_wrapper_or_unsafe(fc: &internal_tracing::function_call::FunctionCall) -> bool {
    fc.is_hidden
        || fc.fn_name.as_ref().map_or(false, |n| {
            n.contains("unsafe_new_") || n.contains("__wrapper__")
        })
}

/// Overrides `debugger_trace_step_index` on contract calls and function calls
/// so they point past wrapper/unsafe boilerplate to the real entrypoint step.
fn apply_real_entrypoint_step_indices(
    contract_calls_map: &mut ContractCallsMap,
    function_calls_map: &mut FunctionCallsMap,
) {
    // Override contract call step indices
    let cc_updates: Vec<(u32, usize)> = contract_calls_map
        .0
        .iter()
        .filter_map(|(id, cc)| {
            find_real_entrypoint_step(cc, function_calls_map).map(|step| (*id, step))
        })
        .collect();

    for (id, step) in cc_updates {
        if let Some(cc) = contract_calls_map.0.get_mut(&id) {
            cc.debugger_trace_step_index = Some(step);
        }
    }

    // Override function call step indices for wrapper/unsafe functions
    let fc_updates: Vec<(u32, usize)> = function_calls_map
        .0
        .iter()
        .filter_map(|(id, fc)| {
            if is_wrapper_or_unsafe(fc) {
                find_real_step_for_function(fc, function_calls_map).map(|step| (*id, step))
            } else {
                None
            }
        })
        .collect();

    for (id, step) in fc_updates {
        if let Some(fc) = function_calls_map.0.get_mut(&id) {
            fc.debugger_trace_step_index = Some(step);
        }
    }
}

/// Recursively searches child contract calls to find the first verified one
/// and returns its real entrypoint step index.
fn find_first_verified_child_entrypoint_step(
    parent_call: &crate::contract_call::ContractCall,
    contract_calls_map: &ContractCallsMap,
    function_calls_map: &FunctionCallsMap,
) -> Option<usize> {
    for child_id in &parent_call.children_call_ids {
        if let Some(child_call) = contract_calls_map.0.get(child_id) {
            // If this child contract call is verified, use it
            if let Some(step) = find_real_entrypoint_step(child_call, function_calls_map) {
                return Some(step);
            }

            // Otherwise, recurse into its children
            if let Some(step) = find_first_verified_child_entrypoint_step(
                child_call,
                contract_calls_map,
                function_calls_map,
            ) {
                return Some(step);
            }
        }
    }
    None
}
