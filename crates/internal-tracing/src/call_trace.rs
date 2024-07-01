use crate::mappings::Mappings;
use anyhow::Result;
use cairo_felt::Felt252;
use cairo_vm::vm::trace::trace_entry::TraceEntry;
use indextree::{Arena, NodeId};
use serde::Serialize;
use std::collections::HashMap;
use verification::cairo_debug_info::{CodeLocation, SierraStatementToCairoDebugInfo};

pub fn get_internal_call_trace(
    mappings: &Mappings,
    relocated_memory: &Vec<Option<Felt252>>,
    vm_trace: &Vec<TraceEntry>,
    sierra_statements_to_cairo_info: Option<&HashMap<usize, SierraStatementToCairoDebugInfo>>,
) -> Result<InternalFnCallTraceEntryNode> {
    let first_vm_trace_entry = vm_trace.first().unwrap();
    let mut current_fp = first_vm_trace_entry.fp;

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
        fp: current_fp,
        cairo_locations: entrypoint_cairo_locations,
        arguments: Vec::new(),
        results: Vec::new(),
    });

    for (i, trace_entry) in vm_trace.iter().enumerate() {
        let new_fp = trace_entry.fp;
        if new_fp > current_fp {
            // new function call
            let sierra_indexes = mappings.get_sierra_indexes_at_pc(&trace_entry.pc);
            let function = sierra_indexes
                .as_ref()
                .and_then(|indexes| indexes.first())
                .and_then(|si| mappings.get_sierra_function_at_sierra_index(&si));
            let cairo_locations = match (sierra_statements_to_cairo_info, sierra_indexes) {
                (Some(sierra_statements_to_cairo_info), Some(sierra_indexes)) => mappings
                    .get_cairo_locations_at_sierra_indexes(
                        sierra_statements_to_cairo_info,
                        &sierra_indexes,
                    ),
                _ => Vec::new(),
            };

            let prev_trace_entry = &vm_trace[i - 1];
            let prev_sierra_index = mappings.get_first_sierra_index_at_pc(&prev_trace_entry.pc);

            let arguments = match prev_sierra_index {
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
                cairo_locations,
                arguments,
                results: Vec::new(),
            };

            tree.add_child(call_entry);
            current_fp = trace_entry.fp;

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
        } else if new_fp < current_fp {
            // return from function
            let sierra_index = mappings.get_first_sierra_index_at_pc(&trace_entry.pc);
            let results = match sierra_index {
                Some(sierra_index) => {
                    mappings.get_results_at_trace_step(relocated_memory, sierra_index, trace_entry)
                }
                None => Vec::new(),
            };

            tree.set_results_to_current_node(results);
            tree.move_to_parent();
            current_fp = trace_entry.fp;

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
        }
    }

    Ok(tree.get_root_serializable())
}

#[derive(Debug, Clone, Serialize)]
pub struct InternalFnCallTraceEntry {
    pub fn_name: Option<String>,
    pub fp: usize,
    pub results: Vec<InternalFnCallIO>,
    pub arguments: Vec<InternalFnCallIO>,
    pub cairo_locations: Vec<CodeLocation>,
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
}
