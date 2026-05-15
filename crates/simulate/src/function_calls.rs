use crate::contract_call::ContractCall;
use crate::contract_calls_map::ContractCallsMap;
use crate::state::ForkStateReader;
use crate::storage_changes::get_storage_changes;
use anyhow::Result;
use blockifier::state::cached_state::CachedState;
use cairo_lang_sierra_to_casm::compiler::CairoProgram;
use cairo_lang_starknet_classes::compiler_version::VersionId;
use cairo_vm::vm::trace::trace_entry::RelocatedTraceEntry;
use internal_tracing::build_debugger_data::ContractCallBuildResult;
use internal_tracing::event_calls_map::EventCallsMap;
use internal_tracing::function_calls_map::FunctionCallsMap;
use internal_tracing::mappings::ClassMappings;
use internal_tracing::utils::compile_sierra_contract_class;
use internal_tracing::ClassDataProvider;
use starknet_rust::core::types::Felt;
use std::collections::HashMap;
use std::sync::Arc;
use tracing::warn;
use walnut_shared::utils::convert_contract_class;
use walnut_shared::utils::is_version_gte;

pub fn create_function_calls_map_generic<D: ClassDataProvider>(
    contract_calls_map: &mut ContractCallsMap,
    next_call_id: &mut u32,
    deepest_function_call_id_with_panic: &mut Option<u32>,
    classes_data: &HashMap<String, D>,
    cached_fork_state: &CachedState<ForkStateReader>,
    with_storage_and_events: bool,
    builder: fn(
        &[Option<Felt>],
        &Vec<RelocatedTraceEntry>,
        &D,
        &mut FunctionCallsMap,
        &mut EventCallsMap,
        &mut u32,
        u32,
        &[u32],
        Arc<ClassMappings>,
    ) -> Result<ContractCallBuildResult>,
) -> Result<(
    FunctionCallsMap,
    Option<EventCallsMap>,
    Option<HashMap<u32, HashMap<String, (Option<String>, String)>>>,
)> {
    let mut function_calls_map = FunctionCallsMap::new();
    let mut event_calls_map = EventCallsMap::default();
    let mut storage_changes_map = HashMap::new();
    let mut prev_nested_level = 0;

    // Per-request cache: keyed by class hash, scoped to this call only.
    // Prevents cross-contamination between simulate (original class) and
    // debug (inline/walnut-debug class) paths that share the same class_hash.
    let mut local_class_cache: HashMap<String, Arc<ClassMappings>> = HashMap::new();

    // Pre-filter calls that have VM data to avoid unnecessary iterations
    let mut calls_with_vm_data: Vec<_> = contract_calls_map
        .0
        .values_mut()
        .filter(|call| call.vm_memory.is_some() && call.vm_trace.is_some())
        .collect();

    for (_idx, call) in calls_with_vm_data.iter_mut().enumerate() {
        // Safety: We already filtered for calls with VM data
        let vm_memory = call.vm_memory.as_ref().unwrap();
        let vm_trace = call.vm_trace.as_ref().unwrap();

        let (class_mappings, class_data) = resolve_class_data(
            call,
            classes_data,
            cached_fork_state,
            &mut local_class_cache,
        )?;

        if let Some(class_mappings) = class_mappings {
            if with_storage_and_events {
                process_storage_changes(
                    call,
                    &class_mappings.casm_program,
                    cached_fork_state,
                    &mut storage_changes_map,
                )?;
            }

            if let Some(class_data) = class_data {
                let compiler_version = class_mappings.compiler_version.clone();
                let result = builder(
                    vm_memory,
                    vm_trace,
                    class_data,
                    &mut function_calls_map,
                    &mut event_calls_map,
                    next_call_id,
                    call.call_id,
                    &call.children_call_ids,
                    class_mappings,
                )?;

                match result {
                    ContractCallBuildResult::Simple {
                        root_function_call_id,
                        panic_id,
                    } => {
                        update_call_metadata(
                            call,
                            Some(compiler_version),
                            Some(class_data),
                            root_function_call_id,
                            &mut function_calls_map,
                        );
                        update_panic_info(
                            call,
                            panic_id,
                            deepest_function_call_id_with_panic,
                            &mut prev_nested_level,
                        );
                    }

                    ContractCallBuildResult::Full {
                        debugger_data,
                        root_function_call_id,
                        panic_id,
                    } => {
                        call.call_debugger_data = Some(debugger_data);
                        update_call_metadata(
                            call,
                            Some(compiler_version),
                            Some(class_data),
                            root_function_call_id,
                            &mut function_calls_map,
                        );
                        update_panic_info(
                            call,
                            panic_id,
                            deepest_function_call_id_with_panic,
                            &mut prev_nested_level,
                        );
                    }
                }
            }
        }

        // Cleanup
        call.vm_trace = None;
        call.vm_memory = None;
    }

    Ok((
        function_calls_map,
        if with_storage_and_events {
            Some(event_calls_map)
        } else {
            None
        },
        if with_storage_and_events {
            Some(storage_changes_map)
        } else {
            None
        },
    ))
}

fn resolve_class_data<'a, D: ClassDataProvider>(
    call: &ContractCall,
    classes_data: &'a HashMap<String, D>,
    cached_fork_state: &CachedState<ForkStateReader>,
    local_class_cache: &mut HashMap<String, Arc<ClassMappings>>,
) -> anyhow::Result<(Option<Arc<ClassMappings>>, Option<&'a D>)> {
    if let Some(class_hash) = &call.class_hash {
        if let Some(class_data) = classes_data.get(class_hash) {
            if let Some(class_mappings) = local_class_cache.get(class_hash).cloned() {
                return Ok((Some(class_mappings), Some(class_data)));
            }

            // Cache miss: compile and build class mappings
            match compile_sierra_contract_class(class_data.get_contract_class(), usize::MAX) {
                Ok((compiled, _, compiler_version)) => {
                    match ClassMappings::new(
                        class_data.get_contract_class(),
                        compiled,
                        compiler_version,
                    ) {
                        Ok(class_mappings) => {
                            let class_mappings = Arc::new(class_mappings);
                            local_class_cache.insert(class_hash.clone(), class_mappings.clone());
                            return Ok((Some(class_mappings), Some(class_data)));
                        }
                        Err(e) => {
                            warn!("ClassMappings build failed: {:?}", e);
                            return Err(anyhow::anyhow!(format!("{:?}", e)));
                        }
                    }
                }
                Err(e) => {
                    warn!("Compilation failed: {:?}", e);
                    return Err(anyhow::anyhow!(format!("{:?}", e)));
                }
            }
        }
    }

    if let Some(class_hash) = call.entry_point.class_hash {
        let class_hash_str = class_hash.to_string();

        if let Some(class_mappings) = local_class_cache.get(&class_hash_str).cloned() {
            return Ok((Some(class_mappings), None));
        }

        if let Ok(contract_class) = cached_fork_state
            .state
            .in_memory_fork_cache
            .borrow()
            .get_contract_class(class_hash)
        {
            if let Some(converted) = convert_contract_class(contract_class) {
                match compile_sierra_contract_class(&converted, usize::MAX) {
                    Ok((compiled, _, compiler_version)) => {
                        match ClassMappings::new(&converted, compiled, compiler_version) {
                            Ok(class_mappings) => {
                                let class_mappings = Arc::new(class_mappings);
                                local_class_cache.insert(class_hash_str, class_mappings.clone());
                                return Ok((Some(class_mappings), None));
                            }
                            Err(e) => return Err(anyhow::anyhow!(format!("{:?}", e))),
                        }
                    }
                    Err(e) => return Err(anyhow::anyhow!(format!("{:?}", e))),
                }
            }
        }
    }

    Ok((None, None))
}

fn process_storage_changes(
    call: &ContractCall,
    casm: &CairoProgram,
    cached_fork_state: &CachedState<ForkStateReader>,
    storage_changes_map: &mut HashMap<u32, HashMap<String, (Option<String>, String)>>,
) -> anyhow::Result<()> {
    if let Some(code_address) = call.entry_point.code_address {
        let vm_trace = call
            .vm_trace
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("Missing VM trace data".to_string()))?;

        let vm_memory = call
            .vm_memory
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("Missing VM memory".to_string()))?;

        if let Some(storage_changes) = get_storage_changes(
            casm,
            vm_trace,
            vm_memory,
            &cached_fork_state
                .state
                .in_memory_fork_cache
                .borrow()
                .get_storage_view(),
            code_address,
        ) {
            storage_changes_map.insert(call.call_id, storage_changes);
        }
    }
    Ok(())
}

fn update_call_metadata<D: ClassDataProvider>(
    call: &mut ContractCall,
    version: Option<VersionId>,
    class_data: Option<&D>,
    root_id: u32,
    function_calls_map: &mut FunctionCallsMap,
) {
    call.function_call_id = Some(root_id);
    call.code_location = function_calls_map
        .0
        .get(&root_id)
        .and_then(|c| c.code_location.clone());

    call.call_debugger_data_available = match version {
        Some(v) if is_version_gte(&v, 2, 8, 2) => class_data
            .and_then(|d| d.get_inline_strategy_class_hash())
            .is_some(),
        _ => true,
    };
}

fn update_panic_info(
    call: &ContractCall,
    panic_id: Option<u32>,
    deepest_function_call_id_with_panic: &mut Option<u32>,
    prev_nested_level: &mut u32,
) {
    if let Some(pid) = panic_id {
        let is_deeper = match *deepest_function_call_id_with_panic {
            Some(_) => call.contract_calls_nesting_level > *prev_nested_level,
            None => true,
        };

        if is_deeper {
            *deepest_function_call_id_with_panic = Some(pid);
            *prev_nested_level = call.contract_calls_nesting_level;
        }
    }
}
