use crate::{mappings::Mappings, utils::is_panic_result};
use anyhow::Result;
use cairo_felt::Felt252;
use cairo_vm::vm::trace::trace_entry::TraceEntry;
use indextree::{Arena, NodeId};
use serde::Serialize;
use std::collections::HashMap;
use verification::cairo_debug_info::{CodeLocation, SierraStatementToCairoDebugInfo};

#[derive(Debug, Serialize)]
pub struct DebuggerExecutionTraceEntry {
    pub sierra_indexes: Vec<usize>,
    pub results: Vec<InternalFnCallIO>,
    pub arguments: Vec<InternalFnCallIO>,
}

pub fn get_internal_call_trace(
    mappings: &Mappings,
    relocated_memory: &Vec<Option<Felt252>>,
    vm_trace: &Vec<TraceEntry>,
    sierra_statements_to_cairo_info: Option<&HashMap<usize, SierraStatementToCairoDebugInfo>>,
) -> Result<(
    InternalFnCallTraceEntryNode,
    Vec<DebuggerExecutionTraceEntry>,
)> {
    let vm_trace_length = vm_trace.len();
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

    let mut tree = InternalFnCallTraceTree::new(InternalFnCallTraceEntry {
        fn_name: entrypoint_function
            .and_then(|f| f.id.debug_name.clone())
            .and_then(|n| Some(n.to_string())),
        fp: prev_fp,
        cairo_locations: entrypoint_cairo_locations,
        arguments: Vec::new(),
        results: Vec::new(),
        is_panic_result: false,
        debugger_execution_trace_step_index: 0,
    });

    // Execution trace of the current contract call that contains data for the debugger
    let mut debugger_execution_trace: Vec<DebuggerExecutionTraceEntry> = Vec::new();
    // Previous Cairo locations: we update this variable only with a non-empty Vec
    let mut prev_cairo_locations: Vec<CodeLocation> = Vec::new();
    // Accumulate arguments for the steps with the same Cairo locations
    let mut arguments_accumulator: Vec<InternalFnCallIO> = Vec::new();
    // Accumulate results for the steps with the same Cairo locations
    let mut results_accumulator: Vec<InternalFnCallIO> = Vec::new();
    // Sierra indexes of the previous step with Cairo locations
    let mut prev_cairo_location_sierra_indexes: Vec<usize> = Vec::new();

    for (i, trace_entry) in vm_trace.iter().enumerate() {
        let new_fp = trace_entry.fp;
        // Active Sierra indexes at the current step
        let sierra_indexes = mappings.get_sierra_indexes_at_pc(&trace_entry.pc);
        let first_sierra_index = sierra_indexes.as_ref().and_then(|indexes| indexes.first());

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
        // Results at the current step (can be empty)
        let mut results: Vec<InternalFnCallIO> = Vec::new();

        if new_fp > prev_fp {
            // If the FP register increases, that means we have entered a nested function call
            let function =
                first_sierra_index.and_then(|si| mappings.get_sierra_function_at_sierra_index(si));

            let prev_trace_entry = &vm_trace[i - 1];
            let prev_sierra_index = mappings.get_first_sierra_index_at_pc(&prev_trace_entry.pc);

            // Get the arguments of the new function call
            arguments = match prev_sierra_index {
                Some(prev_sierra_index) => mappings.get_arguments_at_trace_step(
                    relocated_memory,
                    prev_sierra_index,
                    prev_trace_entry,
                ),
                None => Vec::new(),
            };

            let call_entry = InternalFnCallTraceEntry {
                fn_name: function
                    .and_then(|f| f.id.debug_name.clone())
                    .and_then(|n| Some(n.to_string())),
                fp: new_fp,
                cairo_locations: cairo_locations.clone(),
                arguments: arguments.clone(),
                results: Vec::new(),
                is_panic_result: false,
                debugger_execution_trace_step_index: debugger_execution_trace.len(),
            };

            // Add the nested call and set it as the current node
            tree.add_child(call_entry);

            // Alternative implementation of get_arguments
            // https://docs.cairo-lang.org/how_cairo_works/functions.html#argument
            // TODO: check if is algorithm is correct and fix if needed
            // for argument in call_entry.arguments.iter_mut().rev() {
            //     let type_size = argument.type_size as usize;
            //     let mut values: Vec<String> = Vec::new();
            //     for i in 1..(type_size + 1) {
            //         let addr = trace_entry.ap - 2 - type_size + i;
            //         let value = mappings.common_debug_data.memory_map.get(&addr).unwrap();
            //         values.push(value.clone().to_string());
            //     }
            //     argument.value = values;
            // }
        } else if new_fp < prev_fp {
            // If the FP register decreases, that means we have exited the function call
            let prev_trace_entry = &vm_trace[i - 1];
            let prev_sierra_index = mappings.get_first_sierra_index_at_pc(&prev_trace_entry.pc);

            // Get the results of the function call from which we have just exited
            results = match prev_sierra_index {
                Some(sierra_index) => mappings.get_results_at_trace_step(
                    relocated_memory,
                    sierra_index.clone(),
                    prev_trace_entry,
                ),
                None => Vec::new(),
            };

            tree.set_results_to_current_node(results.clone());
            // Return to the parent function call
            tree.move_to_parent();

            // Alternative implementation of get_results
            // https://docs.cairo-lang.org/how_cairo_works/functions.html#return-values
            // TODO: check if is algorithm is correct and fix if needed
            // for result in exit_function.results.iter().rev() {
            //     let type_size = result.type_size as usize;
            //     let mut values: Vec<String> = Vec::new();
            //     for i in 1..(type_size + 1) {
            //         let addr = trace_entry.ap - type_size + i - offset;
            //         let value = mappings.common_debug_data.memory_map.get(&addr).unwrap();
            //         values.push(value.clone().to_string());
            //     }
            //     result_values.push(values);
            //     offset += type_size;
            // }
            // result_values.reverse();
        } else {
            let current_function = tree.get_current_node_data();
            if cairo_locations.len() > 0 && current_function.cairo_locations.len() == 0 {
                tree.set_cairo_locations_to_current_node(cairo_locations.clone());
            }
        }
        prev_fp = trace_entry.fp;

        if let Some(sierra_indexes) = sierra_indexes {
            // If current step contains Cairo locations
            if cairo_locations.len() > 0 {
                // If current step is the first step with Cairo locations
                if prev_cairo_locations.len() == 0 {
                    // Then accumulate arguments and results
                    results_accumulator = results;
                    arguments_accumulator = arguments;
                // If current step has the same Cairo locations as the last step with Cairo locations
                } else if cairo_locations == prev_cairo_locations {
                    // If there are arguments or results
                    if results.len() > 0 || arguments.len() > 0 {
                        // Then accumulate arguments and results
                        results_accumulator = results;
                        arguments_accumulator = arguments;
                    }
                // If current step has different Cairo locations from previous step with Cairo locations
                } else {
                    // Then add debugger trace entry for the previous step with Cairo locations
                    debugger_execution_trace.push(DebuggerExecutionTraceEntry {
                        sierra_indexes: prev_cairo_location_sierra_indexes.clone(),
                        results: results_accumulator.clone(),
                        arguments: arguments_accumulator.clone(),
                    });
                    // And accumulate arguments and results
                    results_accumulator = results;
                    arguments_accumulator = arguments;
                }
                prev_cairo_location_sierra_indexes = sierra_indexes;
                prev_cairo_locations = cairo_locations;
            }
        }
    }

    tree.set_deepest_panic_result();
    // Add debugger trace entry for the last step with Cairo locations
    debugger_execution_trace.push(DebuggerExecutionTraceEntry {
        sierra_indexes: prev_cairo_location_sierra_indexes,
        results: results_accumulator,
        arguments: arguments_accumulator,
    });

    Ok((tree.get_root_serializable(), debugger_execution_trace))
}

#[derive(Debug, Clone, Serialize)]
pub struct InternalFnCallTraceEntry {
    pub fn_name: Option<String>,
    pub fp: usize,
    pub results: Vec<InternalFnCallIO>,
    pub arguments: Vec<InternalFnCallIO>,
    pub cairo_locations: Vec<CodeLocation>,
    pub is_panic_result: bool,
    pub debugger_execution_trace_step_index: usize,
}

#[derive(Debug, Clone, Serialize)]
pub struct InternalFnCallIO {
    pub type_name: Option<String>,
    pub value: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct InternalFnCallTraceEntryNode {
    pub data: InternalFnCallTraceEntry,
    pub nested_calls: Vec<InternalFnCallTraceEntryNode>,
}

#[derive(Debug)]
struct InternalFnCallTraceTree {
    arena: Arena<InternalFnCallTraceEntry>,
    current_node: NodeId,
    root: NodeId,
}

impl InternalFnCallTraceTree {
    fn new(root_entry: InternalFnCallTraceEntry) -> Self {
        let mut arena = Arena::new();
        let root = arena.new_node(root_entry);
        InternalFnCallTraceTree {
            arena,
            current_node: root,
            root,
        }
    }

    fn find_max_panic_depth(&mut self, node_id: NodeId, depth: usize, max_depth: &mut usize) {
        if let Some(node) = self.arena.get(node_id) {
            let data = &node.get();

            for result in data.results.iter() {
                if is_panic_result(&result.type_name)
                    && result.value[0] == "1"
                    && depth > *max_depth
                {
                    *max_depth = depth;
                }
            }

            let mut child_id = node.first_child();
            while let Some(id) = child_id {
                self.find_max_panic_depth(id, depth + 1, max_depth);
                child_id = self.arena.get(id).and_then(|n| n.next_sibling());
            }
        }
    }

    fn mark_deepest_panic_node(&mut self, node_id: NodeId, depth: usize, max_depth: usize) {
        if let Some(node) = self.arena.get_mut(node_id) {
            let data = &mut node.get_mut();

            data.is_panic_result = false;

            for result in data.results.iter() {
                if depth == max_depth
                    && is_panic_result(&result.type_name)
                    && result.value[0] == "1"
                {
                    data.is_panic_result = true;
                }
            }

            let mut child_id = node.first_child();
            while let Some(id) = child_id {
                self.mark_deepest_panic_node(id, depth + 1, max_depth);
                child_id = self.arena.get(id).and_then(|n| n.next_sibling());
            }
        }
    }

    fn set_deepest_panic_result(&mut self) {
        let mut max_depth = 0;
        self.find_max_panic_depth(self.root, 0, &mut max_depth);
        self.mark_deepest_panic_node(self.root, 0, max_depth);
    }

    fn add_child(&mut self, entry: InternalFnCallTraceEntry) {
        let child = self.arena.new_node(entry);
        self.current_node.append(child, &mut self.arena);
        self.current_node = child;
    }

    fn move_to_parent(&mut self) {
        let mut ancestors = self.current_node.ancestors(&self.arena);
        ancestors.next();
        if let Some(parent) = ancestors.next() {
            self.current_node = parent;
        }
    }

    fn set_results_to_current_node(&mut self, results: Vec<InternalFnCallIO>) {
        if let Some(node) = self.arena.get_mut(self.current_node) {
            let data = node.get_mut();
            data.results = results;
        }
    }

    fn get_serializable(&self, node_id: NodeId) -> InternalFnCallTraceEntryNode {
        let mut nested_calls = Vec::new();
        for child_node_id in node_id.children(&self.arena) {
            nested_calls.push(self.get_serializable(child_node_id));
        }
        InternalFnCallTraceEntryNode {
            data: self.arena[node_id].get().clone(),
            nested_calls,
        }
    }

    fn get_root_serializable(&self) -> InternalFnCallTraceEntryNode {
        self.get_serializable(self.root)
    }

    fn get_current_node_data(&self) -> &InternalFnCallTraceEntry {
        &self.arena[self.current_node].get()
    }

    fn set_cairo_locations_to_current_node(&mut self, cairo_locations: Vec<CodeLocation>) {
        if let Some(node) = self.arena.get_mut(self.current_node) {
            let data = node.get_mut();
            data.cairo_locations = cairo_locations;
        }
    }
}
