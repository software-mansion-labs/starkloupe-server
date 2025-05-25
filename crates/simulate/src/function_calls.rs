use crate::contract_call::ContractCall;
use blockifier::state::cached_state::CachedState;
use cairo_lang_sierra_to_casm::compiler::CairoProgram;
use cairo_lang_starknet_classes::compiler_version::VersionId;
use internal_tracing::build_debugger_data::build_contract_call_debugger_data;
use internal_tracing::build_debugger_data::build_simple_contract_call_debugger_data;
use internal_tracing::function_calls_map::FunctionCallsMap;
use internal_tracing::utils::compile_sierra_contract_class;
use internal_tracing::ClassDebuggerDataWithContractClass;
use internal_tracing::DataWithContractClass;
use std::collections::HashMap;
use tracing::debug;
use tracing::error;
use tracing::warn;
use walnut_shared::utils::convert_contract_class;
use walnut_shared::utils::is_version_gte;

use crate::contract_calls_map::ContractCallsMap;
use crate::state::ForkStateReader;
use crate::storage_changes::get_storage_changes;
use internal_tracing::event_calls_map::EventCallsMap;

pub fn create_simple_function_calls_map(
    contract_calls_map: &mut ContractCallsMap,
    next_call_id: &mut u32,
    deepest_function_call_id_with_panic: &mut Option<u32>,
    classes_data: &HashMap<String, DataWithContractClass>,
    cached_fork_state: &CachedState<ForkStateReader>,
    calculate: bool,
) -> (
    FunctionCallsMap,
    EventCallsMap,
    HashMap<u32, HashMap<String, (Option<String>, String)>>,
) {
    let mut function_calls_map = FunctionCallsMap::new();
    let mut event_calls_map = EventCallsMap::default();
    let mut storage_changes_map: HashMap<u32, HashMap<String, (Option<String>, String)>> =
        HashMap::new();

    let mut prev_contract_call_nested_level = 0;

    for (id, call) in contract_calls_map.0.iter_mut() {
        let (Some(class_hash), Some(vm_memory), Some(vm_trace)) =
            (&call.class_hash, &call.vm_memory, &call.vm_trace)
        else {
            debug!("Not enough data to get internal fn call trace");
            continue;
        };

        let (casm_program, compiler_version, full_class_data) =
            resolve_casm_and_class_data(call, classes_data, cached_fork_state);

        if let (Some(casm_program), Some(code_address)) =
            (&casm_program, call.entry_point.code_address)
        {
            if let Some(storage_changes) = get_storage_changes(
                casm_program,
                vm_trace,
                vm_memory,
                &cached_fork_state
                    .state
                    .in_memory_fork_cache
                    .borrow()
                    .get_storage_view(),
                code_address,
            ) {
                storage_changes_map.insert(*id, storage_changes);
            }
        };

        let (Some(casm_program), Some(full_class_data)) = (casm_program, full_class_data) else {
            continue;
        };

        let Ok((root_function_call_id, deepest_failed_function_call_id)) =
            build_simple_contract_call_debugger_data(
                vm_memory,
                vm_trace,
                full_class_data,
                &mut function_calls_map,
                &mut event_calls_map,
                next_call_id,
                *id,
                &call.children_call_ids,
                casm_program,
                calculate,
            )
        else {
            error!(
                "Failed to get internal fn call trace for class hash {}",
                class_hash
            );
            continue;
        };

        match compiler_version {
            Some(ver) if is_version_gte(&ver, 2, 8, 2) => {
                call.call_debugger_data_available =
                    full_class_data.inline_strategy_class_hash.is_some();
            }
            Some(_) | None => {
                call.call_debugger_data_available = true;
            }
        }

        call.function_call_id = Some(root_function_call_id);
        call.code_location = function_calls_map
            .0
            .get(&root_function_call_id)
            .and_then(|call| call.code_location.as_ref().cloned());

        //We need only one function call with panic, and the one that is from mosted nested
        //contract call
        if let Some(current_id) = deepest_failed_function_call_id {
            let current_nested = call.contract_calls_nesting_level;
            let is_deeper = match deepest_function_call_id_with_panic {
                Some(_) => current_nested > prev_contract_call_nested_level,
                None => true,
            };

            if is_deeper {
                *deepest_function_call_id_with_panic = Some(current_id);
                prev_contract_call_nested_level = current_nested;
            }
        }

        call.vm_trace = None;
        call.vm_memory = None;
    }

    (function_calls_map, event_calls_map, storage_changes_map)
}

fn resolve_casm_and_class_data<'a>(
    call: &ContractCall,
    classes_data: &'a HashMap<String, DataWithContractClass>,
    cached_fork_state: &'a CachedState<ForkStateReader>,
) -> (
    Option<CairoProgram>,
    Option<VersionId>,
    Option<&'a DataWithContractClass>,
) {
    if let Some(class_hash) = &call.class_hash {
        // classes data are verified classes, so if we have it we extract compielr version to
        // cheeck for the inline class hash later
        if let Some(full_class_data) = classes_data.get(class_hash) {
            match compile_sierra_contract_class(&full_class_data.contract_class, usize::MAX) {
                Ok((compiled, _, compiler_version)) => {
                    return (
                        Some(compiled),
                        Some(compiler_version),
                        Some(full_class_data),
                    );
                }
                Err(e) => {
                    warn!("Failed to compile Sierra contract class: {:?}", e);
                    return (None, None, Some(full_class_data));
                }
            }
        }
    }

    // When we don't have class hash in classes_data, that class is not verified, so we will not
    // have inline class hash , and debug data are not available
    if let Some(class_hash) = call.entry_point.class_hash {
        if let Ok(contract_class) = cached_fork_state
            .state
            .in_memory_fork_cache
            .borrow()
            .get_contract_class(class_hash)
        {
            if let Some(converted) = convert_contract_class(contract_class) {
                match compile_sierra_contract_class(&converted, usize::MAX) {
                    Ok((compiled, _, _)) => {
                        return (Some(compiled), None, None);
                    }
                    Err(e) => {
                        warn!("Failed to compile Sierra contract class: {:?}", e);
                        return (None, None, None);
                    }
                }
            }
        }
    }

    (None, None, None)
}

pub fn create_function_calls_map(
    contract_calls_map: &mut ContractCallsMap,
    next_call_id: &mut u32,
    deepest_function_call_id_with_panic: &mut Option<u32>,
    classes_debugger_data: &HashMap<String, ClassDebuggerDataWithContractClass>,
    cached_fork_state: &CachedState<ForkStateReader>,
    calculate: bool,
) -> FunctionCallsMap {
    let mut function_calls_map = FunctionCallsMap::new();

    let mut prev_deepest_nesting = 0;

    for (id, call) in contract_calls_map.0.iter_mut() {
        let (Some(class_hash), Some(vm_memory), Some(vm_trace)) =
            (&call.class_hash, &call.vm_memory, &call.vm_trace)
        else {
            debug!("Not enough data to get internal fn call trace");
            continue;
        };

        let (Some(casm_program), compiler_version, Some(full_class_debugger_data)) =
            resolve_casm_and_class_debugger_data(call, classes_debugger_data, cached_fork_state)
        else {
            continue;
        };

        let Ok((call_debugger_data, root_function_call_id, deepest_failed_function_call_id)) =
            build_contract_call_debugger_data(
                vm_memory,
                vm_trace,
                full_class_debugger_data,
                &mut function_calls_map,
                next_call_id,
                *id,
                &call.children_call_ids,
                casm_program,
                calculate,
            )
        else {
            error!(
                "Failed to get internal fn call trace for class hash {}",
                class_hash
            );
            continue;
        };

        match compiler_version {
            Some(ver) if is_version_gte(&ver, 2, 8, 2) => {
                call.call_debugger_data_available = full_class_debugger_data
                    .inline_strategy_class_hash
                    .is_some();
            }
            Some(_) | None => {
                call.call_debugger_data_available = true;
            }
        }
        call.call_debugger_data = Some(call_debugger_data);
        call.function_call_id = Some(root_function_call_id);
        call.code_location = function_calls_map
            .0
            .get(&root_function_call_id)
            .and_then(|call| call.code_location.as_ref().cloned());

        if let Some(failed_id) = deepest_failed_function_call_id {
            let current_nested = call.contract_calls_nesting_level;
            let is_deeper = match deepest_function_call_id_with_panic {
                Some(_) => current_nested > prev_deepest_nesting,
                None => true,
            };

            if is_deeper {
                *deepest_function_call_id_with_panic = Some(failed_id);
                prev_deepest_nesting = current_nested;
            }
        }

        call.vm_trace = None;
        call.vm_memory = None;
    }

    function_calls_map
}

fn resolve_casm_and_class_debugger_data<'a>(
    call: &ContractCall,
    classes_data: &'a HashMap<String, ClassDebuggerDataWithContractClass>,
    cached_fork_state: &'a CachedState<ForkStateReader>,
) -> (
    Option<CairoProgram>,
    Option<VersionId>,
    Option<&'a ClassDebuggerDataWithContractClass>,
) {
    if let Some(class_hash) = &call.class_hash {
        // classes data are verified classes, so if we have it we extract compielr version to
        // cheeck for the inline class hash later
        if let Some(full_class_data) = classes_data.get(class_hash) {
            match compile_sierra_contract_class(&full_class_data.contract_class, usize::MAX) {
                Ok((compiled, _, compiler_version)) => {
                    return (
                        Some(compiled),
                        Some(compiler_version),
                        Some(full_class_data),
                    );
                }
                Err(e) => {
                    warn!("Failed to compile Sierra contract class: {:?}", e);
                    return (None, None, Some(full_class_data));
                }
            }
        }
    }

    // When we don't have class hash in classes_data, that class is not verified, so we will not
    // have inline class hash , and debug data are not available
    if let Some(class_hash) = call.entry_point.class_hash {
        if let Ok(contract_class) = cached_fork_state
            .state
            .in_memory_fork_cache
            .borrow()
            .get_contract_class(class_hash)
        {
            if let Some(converted) = convert_contract_class(contract_class) {
                match compile_sierra_contract_class(&converted, usize::MAX) {
                    Ok((compiled, _, _)) => {
                        return (Some(compiled), None, None);
                    }
                    Err(e) => {
                        warn!("Failed to compile Sierra contract class: {:?}", e);
                        return (None, None, None);
                    }
                }
            }
        }
    }

    (None, None, None)
}
