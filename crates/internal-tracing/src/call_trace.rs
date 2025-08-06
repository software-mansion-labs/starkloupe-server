use crate::{
    event_call::EventCall,
    event_calls_map::EventCallsMap,
    function_call::FunctionCall,
    function_calls_map::FunctionCallsMap,
    mappings::Mappings,
    utils::{flatten_event_data_struct, get_raw_function_name, is_panic_result},
};
use anyhow::Result;
use cairo_lang_sierra::program::{GenFunction, StatementIdx};
use cairo_lang_starknet_classes::abi::EventField;
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
    pub event_name: Option<String>,
    pub event_members: Vec<EventField>,
    pub event_datas: Vec<DecodedValue>,
}

#[derive(Debug, Serialize, Clone)]
pub struct StorageWrite {
    pub address: Felt,
    pub value: Felt,
}

#[derive(Debug, Serialize, Clone)]
pub enum ESysCall {
    ContractCall(ContractCall),
    EventCall(EventSysCall),
    StorageWrite(StorageWrite),
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

#[derive(Debug, Clone, Serialize)]
pub struct InternalFnCallIO {
    pub type_name: Option<String>,
    pub value: Vec<String>,
}

struct TraceState {
    deepest_panic_function_call_id: Option<u32>,
    deepest_panic_result_level: i32,
    deepest_panic_result_call_id: Option<u32>,
    nesting_level: i32,
    current_call_id: u32,
    root_function_call_id: u32,
    prev_fp: usize,
    prev_cairo_location: Option<CodeLocation>,
    prev_cairo_locations: Vec<CodeLocation>,
    contract_call_index: usize,
    loop_parent_map: HashMap<String, u32>,
    debugger_execution_trace: Option<Vec<DebuggerTraceEntry>>,
}

struct CallTraceBuilder<'a> {
    mappings: &'a Mappings,
    relocated_memory: &'a [Option<Felt>],
    vm_trace: &'a Vec<RelocatedTraceEntry>,
    sierra_statements_to_cairo_debug_info:
        Option<&'a HashMap<usize, SierraStatementToCairoDebugInfo>>,
    function_calls_map: &'a mut FunctionCallsMap,
    event_calls_map: &'a mut EventCallsMap,
    next_call_id: &'a mut u32,
    contract_call_id: u32,
    contract_call_children_ids: &'a [u32],
    debug_mode: bool,
}

impl<'a> CallTraceBuilder<'a> {
    fn new(
        mappings: &'a Mappings,
        relocated_memory: &'a [Option<Felt>],
        vm_trace: &'a Vec<RelocatedTraceEntry>,
        sierra_statements_to_cairo_debug_info: Option<
            &'a HashMap<usize, SierraStatementToCairoDebugInfo>,
        >,
        function_calls_map: &'a mut FunctionCallsMap,
        event_calls_map: &'a mut EventCallsMap,
        next_call_id: &'a mut u32,
        contract_call_id: u32,
        contract_call_children_ids: &'a [u32],
        debug_mode: bool,
    ) -> Self {
        Self {
            mappings,
            relocated_memory,
            vm_trace,
            sierra_statements_to_cairo_debug_info,
            function_calls_map,
            event_calls_map,
            next_call_id,
            contract_call_id,
            contract_call_children_ids,
            debug_mode,
        }
    }

    fn build(&mut self) -> Result<(Option<Vec<DebuggerTraceEntry>>, u32, Option<u32>)> {
        let first_vm_trace_entry = self.vm_trace.first().unwrap();
        let prev_fp = first_vm_trace_entry.fp;

        let entry_point_sierra_indexes = self
            .mappings
            .get_sierra_indexes_at_pc(&first_vm_trace_entry.pc);
        let entry_point_function = entry_point_sierra_indexes
            .as_ref()
            .and_then(|indexes| indexes.first())
            .and_then(|i| self.mappings.get_sierra_function_at_sierra_index(i));

        let entry_point_cairo_locations = if self.debug_mode {
            match (
                self.sierra_statements_to_cairo_debug_info,
                entry_point_sierra_indexes,
            ) {
                (Some(sierra_statements_to_cairo_info), Some(entrypoint_sierra_indexes)) => {
                    self.mappings.get_cairo_locations_at_sierra_indexes(
                        sierra_statements_to_cairo_info,
                        &entrypoint_sierra_indexes,
                    )
                }
                _ => Vec::new(),
            }
        } else {
            Vec::new()
        };

        let mut trace_state = TraceState {
            deepest_panic_function_call_id: None,
            deepest_panic_result_level: -1,
            deepest_panic_result_call_id: None,
            nesting_level: 0,
            current_call_id: self.create_root_function_call(
                prev_fp,
                entry_point_function,
                entry_point_cairo_locations.first().cloned(),
            )?,
            root_function_call_id: *self.next_call_id - 1,
            prev_fp,
            prev_cairo_location: None,
            prev_cairo_locations: Vec::new(),
            contract_call_index: 0,
            loop_parent_map: HashMap::new(),
            debugger_execution_trace: if self.debug_mode {
                Some(Vec::new())
            } else {
                None
            },
        };

        for (i, trace_entry) in self.vm_trace.iter().enumerate() {
            self.process_trace_entry(&mut trace_state, i, trace_entry)?;
            trace_state.prev_fp = trace_entry.fp;
        }

        self.check_root_panic_result(&mut trace_state)?;

        Ok((
            trace_state.debugger_execution_trace,
            trace_state.root_function_call_id,
            trace_state.deepest_panic_function_call_id,
        ))
    }

    fn create_root_function_call(
        &mut self,
        fp: usize,
        entry_point_function: Option<&GenFunction<StatementIdx>>,
        code_location: Option<CodeLocation>,
    ) -> Result<u32> {
        let call_id = *self.next_call_id;
        let root_function_call = FunctionCall {
            call_id,
            parent_call_id: 0,
            children_call_ids: Vec::new(),
            contract_call_id: self.contract_call_id,
            event_call_ids: Vec::new(),
            fn_name: entry_point_function
                .and_then(|f| f.id.debug_name.clone())
                .and_then(|n| get_raw_function_name(n.as_str())),
            fp,
            is_deepest_panic_result: false,
            arguments: Vec::new(),
            arguments_decoded: None,
            results: Vec::new(),
            results_decoded: None,
            code_location, //TODO: will fail, need to set none if it is empty
            debugger_data_available: true,
            debugger_trace_step_index: None,
            is_hidden: true,
        };
        *self.next_call_id += 1;
        self.function_calls_map
            .0
            .insert(call_id, root_function_call);

        Ok(call_id)
    }

    fn process_trace_entry(
        &mut self,
        trace_state: &mut TraceState,
        i: usize,
        trace_entry: &RelocatedTraceEntry,
    ) -> Result<()> {
        // Active Sierra indexes at the current step
        let sierra_indexes = self.mappings.get_sierra_indexes_at_pc(&trace_entry.pc);
        let first_sierra_index = sierra_indexes
            .as_ref()
            .and_then(|indexes| indexes.first())
            .copied();

        // Active Cairo locations at the current step (can be empty)
        let cairo_locations = if self.debug_mode {
            match (self.sierra_statements_to_cairo_debug_info, &sierra_indexes) {
                (Some(info), Some(indexes)) => self
                    .mappings
                    .get_cairo_locations_at_sierra_indexes(info, indexes),
                _ => Vec::new(),
            }
        } else {
            Vec::new()
        };

        // Initialize empty IO data
        let mut arguments = Vec::new();
        let mut arguments_decoded = Vec::new();
        let mut results = Vec::new();
        let mut results_decoded = Vec::new();
        if trace_entry.fp > trace_state.prev_fp {
            let prev_trace_entry = &self.vm_trace[i - 1];
            let prev_sierra_index = self
                .mappings
                .get_first_sierra_index_at_pc(&prev_trace_entry.pc);

            // Get the arguments of the new function call
            (arguments, arguments_decoded) = match prev_sierra_index {
                Some(idx) => self.mappings.get_arguments_at_trace_step(
                    self.relocated_memory,
                    idx,
                    prev_trace_entry,
                ),
                None => (Vec::new(), Vec::new()),
            };

            self.handle_function_entry(
                trace_state,
                i,
                trace_entry,
                first_sierra_index,
                &arguments,
                &arguments_decoded,
            )?;
        } else if trace_entry.fp < trace_state.prev_fp {
            let prev_trace_entry = &self.vm_trace[i - 1];
            let prev_sierra_index = self
                .mappings
                .get_first_sierra_index_at_pc(&prev_trace_entry.pc);

            // Get the results of the function call
            (results, results_decoded) = match prev_sierra_index {
                Some(idx) => self.mappings.get_results_at_trace_step(
                    self.relocated_memory,
                    idx,
                    prev_trace_entry,
                ),
                None => (Vec::new(), Vec::new()),
            };
            self.handle_function_exit(trace_state, i, trace_entry, &results, &results_decoded)?;
        } else if self.debug_mode {
            self.update_current_call_code_location(trace_state, &cairo_locations);
        }

        if self.debug_mode {
            self.process_debugger_trace(
                trace_state,
                trace_entry,
                &sierra_indexes,
                &cairo_locations,
                &arguments,
                &arguments_decoded,
                &results,
                &results_decoded,
            )?;
        }

        self.handle_system_calls(trace_state, trace_entry)
    }

    fn handle_function_entry(
        &mut self,
        trace_state: &mut TraceState,
        i: usize,
        trace_entry: &RelocatedTraceEntry,
        first_sierra_index: Option<usize>,
        arguments: &Vec<InternalFnCallIO>,
        arguments_decoded: &Vec<DecodedValue>,
    ) -> Result<()> {
        let function = first_sierra_index
            .and_then(|si| self.mappings.get_sierra_function_at_sierra_index(&si));

        if let Some(function) = function {
            let debug_fn_name = function.id.debug_name.clone();
            let fn_name = get_raw_function_name(&debug_fn_name.unwrap_or_default());
            if let Some(fn_name) = fn_name {
                if self.is_loop(&fn_name) {
                    if let Some(parent_id) = trace_state.loop_parent_map.get(&fn_name) {
                        trace_state.current_call_id = *parent_id;
                    } else {
                        trace_state
                            .loop_parent_map
                            .insert(fn_name.clone(), trace_state.current_call_id);
                    }
                } else {
                    self.create_new_function_call(
                        trace_state,
                        &fn_name,
                        trace_entry.fp,
                        arguments,
                        arguments_decoded,
                    )?;
                    trace_state.nesting_level += 1;
                }
            }
        }
        Ok(())
    }

    /// Determines if a given function name corresponds to a high-level loop in Sierra.
    ///
    /// High-level loops in Sierra are functions whose names include an `expr-id` enclosed in `[` and `]`.
    /// This pattern appears in debug information
    /// The function checks for the following:
    /// - The presence of `[` and `]` to identify brackets.
    /// - The inclusion of "expr" within the brackets.
    /// - The presence of digits following "expr" within the brackets.
    ///
    /// # Arguments
    /// * `function_name` - A string slice representing the function name.
    ///
    /// # Returns
    /// * `true` if the function name matches the pattern of a high-level loop.
    /// * `false` otherwise.
    fn is_loop(&mut self, function_name: &str) -> bool {
        let mut inside = false;
        let mut buffer = String::new();

        for c in function_name.chars() {
            if c == '[' {
                inside = true;
                buffer.clear();
            } else if c == ']' && inside {
                inside = false;
                if self.is_number_range(&buffer) || self.is_expr_number(&buffer) {
                    return true;
                }
            } else if inside {
                buffer.push(c);
            }
        }

        false
    }

    // Helper: exactly one '-' and numbers on both sides (can be negative, whitespace allowed)
    fn is_number_range(&mut self, s: &str) -> bool {
        let parts: Vec<&str> = s.split('-').collect();
        if parts.len() == 2 {
            let left = parts[0].trim().parse::<i64>();
            let right = parts[1].trim().parse::<i64>();
            return left.is_ok() && right.is_ok();
        }
        false
    }

    // Helper: exprN where N is a number
    fn is_expr_number(&mut self, s: &str) -> bool {
        let s = s.trim();
        if let Some(rest) = s.strip_prefix("expr") {
            return !rest.is_empty() && rest.chars().all(|c| c.is_ascii_digit());
        }
        false
    }

    fn create_new_function_call(
        &mut self,
        trace_state: &mut TraceState,
        fn_name: &str,
        fp: usize,
        arguments: &Vec<InternalFnCallIO>,
        arguments_decoded: &Vec<DecodedValue>,
    ) -> Result<()> {
        // Skip core library functions completely - don't add them to function_calls_map
        if fn_name.starts_with("core::") {
            // For core functions, we need to create a hidden function call to maintain parent tracking
            // but mark it as hidden so it won't be displayed in the UI
            let new_call_id = *self.next_call_id;
            let code_location = if self.debug_mode {
                self.function_calls_map
                    .0
                    .get(&trace_state.current_call_id)
                    .and_then(|call| call.code_location.clone())
            } else {
                None
            };

            let function_call = FunctionCall {
                call_id: new_call_id,
                parent_call_id: trace_state.current_call_id,
                children_call_ids: Vec::new(),
                contract_call_id: self.contract_call_id,
                event_call_ids: Vec::new(),
                fn_name: Some(fn_name.to_string()),
                fp,
                is_deepest_panic_result: false,
                arguments: arguments.clone(),
                arguments_decoded: Some(arguments_decoded.clone()),
                results: Vec::new(),
                results_decoded: None,
                code_location,
                debugger_data_available: false,
                debugger_trace_step_index: None,
                is_hidden: true, // Mark as hidden so it won't be displayed
            };

            *self.next_call_id += 1;
            self.function_calls_map.0.insert(new_call_id, function_call);
            self.function_calls_map
                .0
                .get_mut(&trace_state.current_call_id)
                .unwrap()
                .children_call_ids
                .push(new_call_id);

            trace_state.current_call_id = new_call_id;
            return Ok(());
        }

        let new_call_id = *self.next_call_id;
        let code_location = if self.debug_mode {
            self.function_calls_map
                .0
                .get(&trace_state.current_call_id)
                .and_then(|call| call.code_location.clone())
        } else {
            None
        };

        let function_call = FunctionCall {
            call_id: new_call_id,
            parent_call_id: trace_state.current_call_id,
            children_call_ids: Vec::new(),
            contract_call_id: self.contract_call_id,
            event_call_ids: Vec::new(),
            fn_name: Some(fn_name.to_string()),
            fp,
            is_deepest_panic_result: false,
            arguments: arguments.clone(),
            arguments_decoded: Some(arguments_decoded.clone()),
            results: Vec::new(),
            results_decoded: None,
            code_location,
            debugger_data_available: true,
            debugger_trace_step_index: None,
            is_hidden: false,
        };

        *self.next_call_id += 1;
        self.function_calls_map.0.insert(new_call_id, function_call);
        self.function_calls_map
            .0
            .get_mut(&trace_state.current_call_id)
            .unwrap()
            .children_call_ids
            .push(new_call_id);

        trace_state.current_call_id = new_call_id;

        Ok(())
    }

    fn handle_function_exit(
        &mut self,
        trace_state: &mut TraceState,
        i: usize,
        trace_entry: &RelocatedTraceEntry,
        results: &Vec<InternalFnCallIO>,
        results_decoded: &Vec<DecodedValue>,
    ) -> Result<()> {
        let parent_call_id = self
            .function_calls_map
            .0
            .get(&trace_state.current_call_id)
            .unwrap()
            .parent_call_id;

        if parent_call_id != 0 {
            let parent_call = self.function_calls_map.0.get(&parent_call_id).unwrap();
            if parent_call.fp == trace_entry.fp {
                self.check_for_panic_result(trace_state, results);
                self.update_current_call_results(trace_state, results, results_decoded);
                trace_state.current_call_id = parent_call_id;
                trace_state.nesting_level -= 1;
            }
        }

        Ok(())
    }

    fn check_for_panic_result(
        &mut self,
        trace_state: &mut TraceState,
        results: &[InternalFnCallIO],
    ) {
        for result in results {
            if trace_state.nesting_level > trace_state.deepest_panic_result_level
                && is_panic_result(result.type_name.as_deref())
                && result.value[0] == "1"
            {
                trace_state.deepest_panic_result_level = trace_state.nesting_level;
                trace_state.deepest_panic_result_call_id = Some(trace_state.current_call_id);
                break;
            }
        }
    }

    fn update_current_call_results(
        &mut self,
        trace_state: &mut TraceState,
        results: &[InternalFnCallIO],
        results_decoded: &[DecodedValue],
    ) {
        let current_function_call = self
            .function_calls_map
            .0
            .get_mut(&trace_state.current_call_id)
            .unwrap();
        current_function_call.results = results.to_vec();
        current_function_call.results_decoded = Some(results_decoded.to_vec())
    }

    fn update_current_call_code_location(
        &mut self,
        trace_state: &mut TraceState,
        cairo_locations: &[CodeLocation],
    ) {
        let current_function_call = self
            .function_calls_map
            .0
            .get_mut(&trace_state.current_call_id)
            .unwrap();
        if let Some(location) = cairo_locations.first() {
            if current_function_call.code_location.is_none() {
                current_function_call.code_location = Some(location.clone());
            }
        }
    }

    fn process_debugger_trace(
        &mut self,
        trace_state: &mut TraceState,
        trace_entry: &RelocatedTraceEntry,
        sierra_indexes: &Option<Vec<usize>>,
        cairo_locations: &[CodeLocation],
        arguments: &[InternalFnCallIO],
        arguments_decoded: &[DecodedValue],
        results: &[InternalFnCallIO],
        results_decoded: &[DecodedValue],
    ) -> Result<()> {
        if let Some(sierra_indexes) = sierra_indexes {
            for &sierra_index in sierra_indexes {
                let locations = match self.sierra_statements_to_cairo_debug_info {
                    Some(info) => self
                        .mappings
                        .get_cairo_locations_at_sierra_index(info, sierra_index),
                    None => Vec::new(),
                };

                for (location_index, cairo_location) in locations.iter().enumerate() {
                    let should_add_entry = match trace_state.prev_cairo_location {
                        None => true,
                        Some(ref prev) => {
                            cairo_location != prev && locations != trace_state.prev_cairo_locations
                        }
                    };

                    if should_add_entry {
                        if let Some(ref mut debugger_trace) = trace_state.debugger_execution_trace {
                            debugger_trace.push(DebuggerTraceEntry::WithLocation(
                                DebuggerTraceEntryWithLocation {
                                    sierra_index,
                                    results: results.to_vec(),
                                    arguments: arguments.to_vec(),
                                    results_decoded: if self.debug_mode {
                                        Some(results_decoded.to_vec())
                                    } else {
                                        None
                                    },
                                    arguments_decoded: if self.debug_mode {
                                        Some(arguments_decoded.to_vec())
                                    } else {
                                        None
                                    },
                                    location_index,
                                    contract_call_id: self.contract_call_id,
                                    fp: trace_entry.fp,
                                    function_call_id: trace_state.current_call_id,
                                },
                            ));
                        }
                    }

                    trace_state.prev_cairo_location = Some(cairo_location.clone());
                }
                trace_state.prev_cairo_locations = locations.clone();
            }
        }
        Ok(())
    }

    fn handle_system_calls(
        &mut self,
        trace_state: &mut TraceState,
        trace_entry: &RelocatedTraceEntry,
    ) -> Result<()> {
        if let Some(system_call) = self
            .mappings
            .mappings_get_system_call_at_trace_step(self.relocated_memory, trace_entry)
        {
            match system_call {
                ESysCall::ContractCall(_) => {
                    self.function_calls_map
                        .0
                        .get_mut(&trace_state.current_call_id)
                        .unwrap()
                        .children_call_ids
                        .push(self.contract_call_children_ids[trace_state.contract_call_index]);
                    if self.debug_mode {
                        if let Some(ref mut debugger_trace) = trace_state.debugger_execution_trace {
                            debugger_trace.push(DebuggerTraceEntry::WithContractCall(
                                DebuggerTraceEntryWithContractCall {
                                    contract_call_id: self.contract_call_children_ids
                                        [trace_state.contract_call_index],
                                    reason: None,
                                },
                            ));
                        }
                    }
                    trace_state.contract_call_index += 1;
                }
                ESysCall::EventCall(event) => {
                    if let (Some(current_function_call), Some(event_name)) = (
                        self.function_calls_map
                            .0
                            .get_mut(&trace_state.current_call_id),
                        event.event_name,
                    ) {
                        let new_event_call_id = *self.next_call_id;
                        current_function_call.event_call_ids.push(new_event_call_id);
                        current_function_call
                            .children_call_ids
                            .push(new_event_call_id);

                        let datas = flatten_event_data_struct(event.event_datas);
                        let event_call = EventCall {
                            call_id: new_event_call_id,
                            contract_call_id: current_function_call.contract_call_id,
                            function_call_id: current_function_call.call_id,
                            name: event_name,
                            selector: Some(event.event_selector.to_fixed_hex_string()),
                            members: event.event_members,
                            datas: Some(datas),
                            is_hidden: false,
                        };

                        *self.next_call_id += 1;
                        self.event_calls_map.0.insert(new_event_call_id, event_call);
                    }
                }
                _ => {}
            }
        }
        Ok(())
    }

    fn check_root_panic_result(&mut self, trace_state: &mut TraceState) -> Result<()> {
        if trace_state.deepest_panic_result_call_id.is_none() {
            let last_trace_entry = self.vm_trace.last().unwrap();
            let last_sierra_index = self
                .mappings
                .get_first_sierra_index_at_pc(&last_trace_entry.pc);

            let root_call_results = match last_sierra_index {
                Some(idx) => self.mappings.get_results_at_trace_step(
                    self.relocated_memory,
                    idx,
                    last_trace_entry,
                ),
                None => (Vec::new(), Vec::new()),
            };

            for result in root_call_results.0 {
                if is_panic_result(result.type_name.as_deref()) && result.value[0] == "1" {
                    trace_state.deepest_panic_result_call_id =
                        Some(trace_state.root_function_call_id);
                    break;
                }
            }
        }

        if let Some(panic_call_id) = trace_state.deepest_panic_result_call_id {
            let panic_call = self.function_calls_map.0.get_mut(&panic_call_id).unwrap();
            trace_state.deepest_panic_function_call_id = Some(panic_call.call_id);
        }

        Ok(())
    }
}

pub fn get_simple_internal_call_trace(
    mappings: &Mappings,
    relocated_memory: &[Option<Felt>],
    vm_trace: &Vec<RelocatedTraceEntry>,
    function_calls_map: &mut FunctionCallsMap,
    event_calls_map: &mut EventCallsMap,
    next_call_id: &mut u32,
    contract_call_id: u32,
    contract_call_children_ids: &[u32],
) -> Result<(u32, Option<u32>)> {
    let mut builder = CallTraceBuilder::new(
        mappings,
        relocated_memory,
        vm_trace,
        None,
        function_calls_map,
        event_calls_map,
        next_call_id,
        contract_call_id,
        contract_call_children_ids,
        false,
    );

    let (_, root_call_id, panic_call_id) = builder.build()?;
    Ok((root_call_id, panic_call_id))
}

pub fn get_internal_call_trace(
    mappings: &Mappings,
    relocated_memory: &[Option<Felt>],
    vm_trace: &Vec<RelocatedTraceEntry>,
    sierra_statements_to_cairo_debug_info: Option<&HashMap<usize, SierraStatementToCairoDebugInfo>>,
    function_calls_map: &mut FunctionCallsMap,
    next_call_id: &mut u32,
    contract_call_id: u32,
    contract_call_children_ids: &[u32],
) -> Result<(Vec<DebuggerTraceEntry>, u32, Option<u32>)> {
    let mut events_calls_map = EventCallsMap::default();
    let mut builder = CallTraceBuilder::new(
        mappings,
        relocated_memory,
        vm_trace,
        sierra_statements_to_cairo_debug_info,
        function_calls_map,
        &mut events_calls_map, // Pass a default if not used
        next_call_id,
        contract_call_id,
        contract_call_children_ids,
        true,
    );

    let (debug_trace, root_call_id, panic_call_id) = builder.build()?;
    Ok((debug_trace.unwrap(), root_call_id, panic_call_id))
}
