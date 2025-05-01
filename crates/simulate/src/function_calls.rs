use blockifier::state::cached_state::CachedState;
use internal_tracing::build_debugger_data::build_contract_call_debugger_data;
use internal_tracing::function_calls_map::FunctionCallsMap;
use internal_tracing::utils::compile_sierra_contract_class;
use internal_tracing::ClassDebuggerDataWithContractClass;
use std::collections::HashMap;
use tracing::debug;
use tracing::error;
use tracing::warn;
use walnut_shared::utils::convert_contract_class;

use crate::contract_calls_map::ContractCallsMap;
use crate::state::ForkStateReader;
use crate::storage_changes::get_storage_changes;
use internal_tracing::event_calls_map::EventCallsMap;

pub fn create_function_calls_map(
    contract_calls_map: &mut ContractCallsMap,
    next_call_id: &mut u32,
    classes_debugger_data: &HashMap<String, ClassDebuggerDataWithContractClass>,
    cached_fork_state: &CachedState<ForkStateReader>,
) -> (
    FunctionCallsMap,
    EventCallsMap,
    HashMap<u32, HashMap<String, (Option<String>, String)>>,
) {
    let mut function_calls_map = FunctionCallsMap::new();
    let mut event_calls_map = EventCallsMap::default();
    let mut storage_changes_map: HashMap<u32, HashMap<String, (Option<String>, String)>> =
        HashMap::new();

    for (id, call) in contract_calls_map.0.iter_mut() {
        let result = match (&call.class_hash, &call.vm_memory, &call.vm_trace) {
            (Some(class_hash), Some(vm_memory), Some(vm_trace)) => {
                let (casm_program, full_class_debugger_data) = match classes_debugger_data
                    .get(class_hash)
                {
                    Some(full_class_debugger_data) => (
                        compile_sierra_contract_class(
                            full_class_debugger_data.contract_class.clone(),
                            usize::MAX,
                        )
                        .map_err(|e| {
                            warn!("Failed to compile sierra contract class: {:?}", e);
                            e
                        })
                        .ok(),
                        Some(full_class_debugger_data),
                    ),
                    None => {
                        if let Some(class_hash) = call.entry_point.class_hash {
                            if let Some(contract_class) = cached_fork_state
                                .state
                                .in_memory_fork_cache
                                .borrow()
                                .get_contract_class(class_hash)
                                .ok()
                            {
                                match convert_contract_class(contract_class) {
                                    Some(contract_class) => (
                                        compile_sierra_contract_class(contract_class, usize::MAX)
                                            .map_err(|e| {
                                                warn!(
                                                    "Failed to compile sierra contract class: {:?}",
                                                    e
                                                );
                                                e
                                            })
                                            .ok(),
                                        None,
                                    ),
                                    None => (None, None),
                                }
                            } else {
                                (None, None)
                            }
                        } else {
                            (None, None)
                        }
                    }
                };

                if let (Some(casm_program), Some(code_address)) =
                    (&casm_program, call.entry_point.code_address)
                {
                    if let Some(storage_changes) = get_storage_changes(
                        &casm_program,
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

                match (casm_program, full_class_debugger_data) {
                    (Some(casm_program), Some(full_class_debugger_data)) => {
                        match build_contract_call_debugger_data(
                            vm_memory,
                            vm_trace,
                            full_class_debugger_data,
                            &mut function_calls_map,
                            &mut event_calls_map,
                            next_call_id,
                            *id,
                            &call.children_call_ids,
                            casm_program,
                        ) {
                            Ok((call_debugger_data, root_function_call_id)) => {
                                Some((call_debugger_data, root_function_call_id))
                            }
                            Err(e) => {
                                error!(
                                    "Failed to get internal fn call trace for class hash {}: {:?}",
                                    class_hash, e
                                );
                                None
                            }
                        }
                    }
                    _ => None,
                }
            }
            _ => {
                debug!("Not enough data to get internal fn call trace");
                None
            }
        };

        if let Some((call_debugger_data, root_function_call_id)) = result {
            call.call_debugger_data = Some(call_debugger_data);
            call.function_call_id = Some(root_function_call_id);
            call.code_location = function_calls_map
                .0
                .get(&root_function_call_id)
                .and_then(|call| call.code_location.clone());
        }

        call.vm_trace = None;
        call.vm_memory = None;
    }

    (function_calls_map, event_calls_map, storage_changes_map)
}
