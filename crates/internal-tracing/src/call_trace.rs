use crate::{
    event_call::EventCall,
    event_calls_map::EventCallsMap,
    function_call::FunctionCall,
    function_calls_map::FunctionCallsMap,
    mappings::Mappings,
    utils::{find_event_by_selector, get_raw_function_name, is_loop, is_panic_result},
};
use anyhow::Result;
use cairo_vm::vm::trace::trace_entry::RelocatedTraceEntry;
use data_decoder::DecodedValue;
use serde::Serialize;
use starknet::core::types::Felt;
use std::collections::HashMap;
use verification::{CodeLocation, SierraStatementToCairoDebugInfo};

#[derive(Debug, Serialize, Clone)]
pub struct ContractCall {
    pub contract_address: String,
    pub function_selector: String,
}

#[derive(Debug, Serialize, Clone)]
pub struct EventSysCall {
    pub event_selector: Felt,
    pub event_key: Felt,
}

#[derive(Debug, Serialize, Clone)]
pub enum ESysCall {
    ContractCall(ContractCall),
    EventCall(EventSysCall),
}

#[derive(Debug, Serialize)]
pub enum DebuggerTraceEntry {
    WithLocation(DebuggerTraceEntryWithLocation),
    WithContractCall(DebuggerTraceEntryWithContractCall),
}

#[derive(Debug, Serialize)]
pub struct DebuggerTraceEntryWithContractCall {
    pub contract_call_id: u32,
    pub reason: Option<String>,
}

#[derive(Debug, Serialize, Clone)]
pub struct DebuggerTraceEntryWithLocation {
    pub sierra_index: usize,
    pub location_index: usize,
    pub results: Vec<InternalFnCallIO>,
    pub arguments: Vec<InternalFnCallIO>,
    pub results_decoded: Option<Vec<DecodedValue>>,
    pub arguments_decoded: Option<Vec<DecodedValue>>,
    pub contract_call_id: u32,
    pub fp: usize,
    pub function_call_id: u32,
}

pub fn get_internal_call_trace(
    mappings: &Mappings,
    relocated_memory: &[Option<Felt>],
    vm_trace: &Vec<RelocatedTraceEntry>,
    sierra_statements_to_cairo_info: Option<&HashMap<usize, SierraStatementToCairoDebugInfo>>,
    function_calls_map: &mut FunctionCallsMap,
    event_calls_map: &mut EventCallsMap,
    next_call_id: &mut u32,
    contract_call_id: u32,
    contract_call_children_ids: &[u32],
) -> Result<(Vec<DebuggerTraceEntry>, u32)> {
    let first_vm_trace_entry = vm_trace.first().unwrap();
    let mut prev_fp = first_vm_trace_entry.fp;

    let entrypoint_sierra_indexes = mappings.get_sierra_indexes_at_pc(&first_vm_trace_entry.pc);
    let entrypoint_function = entrypoint_sierra_indexes
        .as_ref()
        .and_then(|indexes| indexes.first())
        .and_then(|i| mappings.get_sierra_function_at_sierra_index(i));

    let entrypoint_cairo_locations =
        match (sierra_statements_to_cairo_info, entrypoint_sierra_indexes) {
            (Some(sierra_statements_to_cairo_info), Some(entrypoint_sierra_indexes)) => mappings
                .get_cairo_locations_at_sierra_indexes(
                    sierra_statements_to_cairo_info,
                    &entrypoint_sierra_indexes,
                ),
            _ => Vec::new(),
        };

    let mut deepest_panic_result_level = -1;
    let mut deepest_panic_result_call_id: Option<u32> = None;
    let mut nesting_level = 0;

    let root_function_call_id = *next_call_id;
    let root_function_call = FunctionCall {
        call_id: root_function_call_id,
        parent_call_id: 0,
        children_call_ids: Vec::new(),
        contract_call_id,
        event_call_ids: Vec::new(),
        fn_name: entrypoint_function
            .and_then(|f| f.id.debug_name.clone())
            .and_then(|n| get_raw_function_name(n.as_str())),
        fp: prev_fp,
        is_deepest_panic_result: false,
        arguments: Vec::new(),
        arguments_decoded: None,
        results: Vec::new(),
        results_decoded: None,
        code_location: entrypoint_cairo_locations.first().cloned(),
        debugger_trace_step_index: None,
        is_hidden: true, // Hide root function call
    };
    *next_call_id += 1;
    function_calls_map
        .0
        .insert(root_function_call_id, root_function_call);
    let mut current_call_id = root_function_call_id;

    // Execution trace of the current contract call that contains data for the debugger
    let mut debugger_execution_trace: Vec<DebuggerTraceEntry> = Vec::new();
    // Previous Cairo location: we update this variable only with a Some CodeLocation
    let mut prev_cairo_location: Option<CodeLocation> = None;

    let mut prev_cairo_locations: Vec<CodeLocation> = Vec::new();

    let mut contract_call_index = 0;

    let mut loop_parent_map: HashMap<String, u32> = HashMap::new();
    for (i, trace_entry) in vm_trace.iter().enumerate() {
        // Active Sierra indexes at the current step
        let sierra_indexes = mappings.get_sierra_indexes_at_pc(&trace_entry.pc);
        let first_sierra_index = sierra_indexes
            .as_ref()
            .and_then(|indexes| indexes.first())
            .copied();

        // Active Cairo locations at the current step (can be empty)
        let cairo_locations = match (sierra_statements_to_cairo_info, &sierra_indexes) {
            (Some(sierra_statements_to_cairo_info), Some(sierra_indexes)) => mappings
                .get_cairo_locations_at_sierra_indexes(
                    sierra_statements_to_cairo_info,
                    sierra_indexes,
                ),
            _ => Vec::new(),
        };

        // Arguments at the current step (can be empty)
        let mut arguments: Vec<InternalFnCallIO> = Vec::new();
        let mut arguments_decoded: Vec<DecodedValue> = Vec::new();
        // Results at the current step (can be empty)
        let mut results: Vec<InternalFnCallIO> = Vec::new();
        let mut results_decoded: Vec<DecodedValue> = Vec::new();

        if trace_entry.fp > prev_fp {
            // If the FP register increases, that means we have entered a nested function call
            let function =
                first_sierra_index.and_then(|si| mappings.get_sierra_function_at_sierra_index(&si));

            let prev_trace_entry = &vm_trace[i - 1];
            let prev_sierra_index = mappings.get_first_sierra_index_at_pc(&prev_trace_entry.pc);

            // Get the arguments of the new function call
            (arguments, arguments_decoded) = match prev_sierra_index {
                Some(prev_sierra_index) => mappings.get_arguments_at_trace_step(
                    relocated_memory,
                    prev_sierra_index,
                    prev_trace_entry,
                ),
                None => (Vec::new(), Vec::new()),
            };

            if let Some(function) = function {
                let debug_fn_name = function.id.debug_name.clone();
                let fn_name = get_raw_function_name(&debug_fn_name.unwrap_or_default());

                if let Some(fn_name) = fn_name {
                    if is_loop(&fn_name) {
                        if let Some(parent_id) = loop_parent_map.get(&fn_name) {
                            current_call_id = *parent_id;
                        } else {
                            loop_parent_map.insert(fn_name.clone(), current_call_id);
                        }
                    } else {
                        let new_function_call_id = *next_call_id;

                        let function_call = FunctionCall {
                            call_id: new_function_call_id,
                            parent_call_id: current_call_id,
                            children_call_ids: Vec::new(),
                            contract_call_id,
                            event_call_ids: Vec::new(),
                            fn_name: Some(fn_name),
                            fp: trace_entry.fp,
                            is_deepest_panic_result: false,
                            arguments: arguments.clone(),
                            arguments_decoded: Some(arguments_decoded.clone()),
                            results: Vec::new(),
                            results_decoded: None,
                            code_location: cairo_locations.first().cloned(),
                            debugger_trace_step_index: None,
                            is_hidden: false,
                        };
                        *next_call_id += 1;
                        function_calls_map
                            .0
                            .insert(new_function_call_id, function_call);
                        function_calls_map
                            .0
                            .get_mut(&current_call_id)
                            .unwrap()
                            .children_call_ids
                            .push(new_function_call_id);
                        current_call_id = new_function_call_id;

                        nesting_level += 1;
                    }
                }
            }
        } else if trace_entry.fp < prev_fp {
            // If the FP register decreases, that means we have exited the function call
            let prev_trace_entry = &vm_trace[i - 1];
            let prev_sierra_index = mappings.get_first_sierra_index_at_pc(&prev_trace_entry.pc);

            // Get the results of the function call from which we have just exited
            (results, results_decoded) = match prev_sierra_index {
                Some(sierra_index) => mappings.get_results_at_trace_step(
                    relocated_memory,
                    sierra_index,
                    prev_trace_entry,
                ),
                None => (Vec::new(), Vec::new()),
            };

            let parent_call_id = function_calls_map
                .0
                .get(&current_call_id)
                .unwrap()
                .parent_call_id;

            if parent_call_id != 0 {
                let parent_call = function_calls_map.0.get(&parent_call_id).unwrap();

                if parent_call.fp == trace_entry.fp {
                    for result in results.iter() {
                        if nesting_level > deepest_panic_result_level
                            && is_panic_result(result.type_name.as_deref())
                            && result.value[0] == "1"
                        {
                            deepest_panic_result_level = nesting_level;
                            deepest_panic_result_call_id = Some(current_call_id);
                            break;
                        }
                    }

                    let current_function_call =
                        function_calls_map.0.get_mut(&current_call_id).unwrap();
                    current_function_call.results = results.clone();
                    current_function_call.results_decoded = Some(results_decoded.clone());

                    // Return to the parent function call
                    current_call_id = parent_call_id;

                    nesting_level -= 1;
                }
            }
        } else {
            let current_function_call = function_calls_map.0.get_mut(&current_call_id).unwrap();
            if let Some(cairo_location) = cairo_locations.first() {
                if current_function_call.code_location.is_none() {
                    current_function_call.code_location = Some(cairo_location.clone());
                }
            }
        }

        if let Some(sierra_indexes) = sierra_indexes {
            for sierra_index in sierra_indexes {
                let cairo_locations = match sierra_statements_to_cairo_info {
                    Some(sierra_statements_to_cairo_info) => mappings
                        .get_cairo_locations_at_sierra_index(
                            sierra_statements_to_cairo_info,
                            sierra_index,
                        ),
                    _ => Vec::new(),
                };
                for (location_index, cairo_location) in cairo_locations.iter().enumerate() {
                    // If current step is the first step with Cairo location
                    if prev_cairo_location.is_none() {
                        debugger_execution_trace.push(DebuggerTraceEntry::WithLocation(
                            DebuggerTraceEntryWithLocation {
                                sierra_index,
                                results: results.clone(),
                                arguments: arguments.clone(),
                                results_decoded: Some(results_decoded.clone()),
                                arguments_decoded: Some(arguments_decoded.clone()),
                                location_index,
                                contract_call_id,
                                fp: trace_entry.fp,
                                function_call_id: current_call_id,
                            },
                        ));
                        // If current step has the same Cairo location as the last step with Cairo location
                    } else if cairo_location == &prev_cairo_location.unwrap()
                        || cairo_locations == prev_cairo_locations
                    {
                        // If there are arguments or results
                        if !results.is_empty() || !arguments.is_empty() {
                            // Find the last step with Cairo location (not WithContractCall) and update it with the current results and arguments
                            if let Some(DebuggerTraceEntry::WithLocation(last_with_location)) =
                                debugger_execution_trace.iter_mut().rev().find(|entry| {
                                    matches!(entry, DebuggerTraceEntry::WithLocation(_))
                                })
                            {
                                last_with_location.results = results.clone();
                                last_with_location.arguments = arguments.clone();
                                last_with_location.results_decoded = Some(results_decoded.clone());
                                last_with_location.arguments_decoded =
                                    Some(arguments_decoded.clone());
                            }
                        }
                    // If current step has a different Cairo location than the last step with Cairo location
                    } else {
                        debugger_execution_trace.push(DebuggerTraceEntry::WithLocation(
                            DebuggerTraceEntryWithLocation {
                                sierra_index,
                                results: results.clone(),
                                arguments: arguments.clone(),
                                results_decoded: Some(results_decoded.clone()),
                                arguments_decoded: Some(arguments_decoded.clone()),
                                location_index,
                                contract_call_id,
                                fp: trace_entry.fp,
                                function_call_id: current_call_id,
                            },
                        ));
                    }

                    prev_cairo_location = Some(cairo_location.clone());
                }
                prev_cairo_locations = cairo_locations.clone();
            }
        }

        if let Some(system_call) =
            mappings.get_system_call_at_trace_step(relocated_memory, trace_entry)
        {
            match system_call {
                ESysCall::ContractCall(_contract) => {
                    function_calls_map
                        .0
                        .get_mut(&current_call_id)
                        .unwrap()
                        .children_call_ids
                        .push(contract_call_children_ids[contract_call_index]);
                    debugger_execution_trace.push(DebuggerTraceEntry::WithContractCall(
                        DebuggerTraceEntryWithContractCall {
                            contract_call_id: contract_call_children_ids[contract_call_index],
                            reason: None,
                        },
                    ));
                    contract_call_index += 1;
                }
                ESysCall::EventCall(event) => {
                    let new_event_call_id = *next_call_id;

                    // Get the current function call
                    if let Some(current_function_call) =
                        function_calls_map.0.get_mut(&current_call_id)
                    {
                        // Find event name and members
                        let (event_name, event_members) =
                            find_event_by_selector(&mappings.events, event.event_selector);
                        // Ensure both event_name is Some and event_members is not empty before proceeding
                        if let (Some(event_name), false) = (event_name, event_members.is_empty()) {
                            let mut keys = Vec::new();
                            let mut datas = Vec::new();
                            // TODO: In this case events are enum in worst case, so we need to
                            // decode enums
                            current_function_call.event_call_ids.push(new_event_call_id);

                            let event_call = EventCall {
                                call_id: new_event_call_id,
                                contract_call_id: current_function_call.contract_call_id,
                                function_call_id: current_function_call.call_id,
                                name: event_name,
                                selector: Some(event.event_selector.to_fixed_hex_string()),
                                members: event_members,
                                keys,
                                datas,
                                is_hidden: false,
                            };
                            current_function_call
                                .children_call_ids
                                .push(new_event_call_id);
                            *next_call_id += 1;
                            event_calls_map.0.insert(new_event_call_id, event_call);
                        }
                    }
                }
            }
        }

        prev_fp = trace_entry.fp;
    }

    // If no panic result was found, check the root function call
    if deepest_panic_result_call_id.is_none() {
        let last_trace_entry = &vm_trace[vm_trace.len() - 1];
        let last_sierra_index = mappings.get_first_sierra_index_at_pc(&last_trace_entry.pc);

        let root_call_results = match last_sierra_index {
            Some(sierra_index) => {
                mappings.get_results_at_trace_step(relocated_memory, sierra_index, last_trace_entry)
            }
            None => (Vec::new(), Vec::new()),
        };

        for result in root_call_results.0.iter() {
            if is_panic_result(result.type_name.as_deref()) && result.value[0] == "1" {
                deepest_panic_result_call_id = Some(root_function_call_id);
                break;
            }
        }
    }

    if let Some(deepest_panic_result_call_id) = &deepest_panic_result_call_id {
        let deepest_panic_call = function_calls_map
            .0
            .get_mut(deepest_panic_result_call_id)
            .unwrap();
        deepest_panic_call.is_deepest_panic_result = true;
    }

    Ok((debugger_execution_trace, root_function_call_id))
}

#[derive(Debug, Clone, Serialize)]
pub struct InternalFnCallIO {
    pub type_name: Option<String>,
    pub value: Vec<String>,
}
