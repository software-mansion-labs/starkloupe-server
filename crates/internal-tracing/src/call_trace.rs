use crate::{mappings::Mappings, utils::is_panic_result};
use anyhow::Result;
use cairo_vm::vm::trace::trace_entry::RelocatedTraceEntry;
use indextree::{Arena, NodeId};
use serde::Serialize;
use serde_json::json;
use serde_json::Value;
use starknet::core::types::Felt;
use std::collections::HashMap;
use verification::{CodeLocation, SierraStatementToCairoDebugInfo};
use walnut_shared::{get_contract_call_id, get_internal_function_call_id};

#[derive(Debug, Serialize, Clone)]
pub struct ContractCall {
    pub contract_address: String,
    pub function_selector: String,
}

#[derive(Debug, Serialize, Clone)]
pub enum ESysCall {
    ContractCall(ContractCall),
}

#[derive(Debug, Serialize)]
pub enum DebuggerExecutionTraceEntry {
    WithLocation(DebuggerExecutionTraceEntryWithLocation),
    WithContractCall(DebuggerExecutionTraceEntryWithContractCall),
}

#[derive(Debug, Serialize)]
pub struct DebuggerExecutionTraceEntryWithContractCall {
    pub contract_call: ContractCall,
    pub contract_call_id: String,
}

#[derive(Debug, Serialize)]
pub struct DebuggerExecutionTraceEntryWithLocation {
    pub sierra_index: usize,
    pub location_index: usize,
    pub results: Option<Value>,
    pub arguments: Option<Value>,
    pub function_id: Option<String>,
}

pub fn get_internal_call_trace(
    mappings: &Mappings,
    relocated_memory: &Vec<Option<Felt>>,
    vm_trace: &Vec<RelocatedTraceEntry>,
    sierra_statements_to_cairo_info: Option<&HashMap<usize, SierraStatementToCairoDebugInfo>>,
    parent_contract_call_id: &String,
) -> Result<(
    InternalFnCallTraceEntryNode,
    Vec<DebuggerExecutionTraceEntry>,
)> {
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

    let mut tree = InternalFnCallTraceTree::new(
        InternalFnCallTraceEntry {
            id: get_internal_function_call_id(&parent_contract_call_id, first_vm_trace_entry.fp),
            fn_name: entrypoint_function
                .and_then(|f| f.id.debug_name.clone())
                .and_then(|n| Some(n.to_string())),
            fp: prev_fp,
            cairo_location: entrypoint_cairo_locations.first().cloned(),
            arguments: Vec::new(),
            arguments_decoded: None,
            results: Vec::new(),
            results_decoded: None,
            is_panic_result: false,
            debugger_execution_trace_step_index: 0,
            nested_calls_ids: Vec::new(),
        },
        parent_contract_call_id.to_owned(),
    );

    // Execution trace of the current contract call that contains data for the debugger
    let mut debugger_execution_trace: Vec<DebuggerExecutionTraceEntry> = Vec::new();
    // Previous Cairo location: we update this variable only with a Some CodeLocation
    let mut prev_cairo_location: Option<CodeLocation> = None;

    let mut prev_cairo_locations: Vec<CodeLocation> = Vec::new();

    let mut contract_call_index = 0;

    for (i, trace_entry) in vm_trace.iter().enumerate() {
        let new_fp = trace_entry.fp;

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
        let mut arguments_decoded: Vec<Value> = Vec::new();
        // Results at the current step (can be empty)
        let mut results: Vec<InternalFnCallIO> = Vec::new();
        let mut results_decoded: Vec<Value> = Vec::new();

        if new_fp > prev_fp {
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

            let call_entry = InternalFnCallTraceEntry {
                id: get_internal_function_call_id(&parent_contract_call_id, new_fp),
                fn_name: function
                    .and_then(|f| f.id.debug_name.clone())
                    .and_then(|n| Some(n.to_string())),
                fp: new_fp,
                cairo_location: cairo_locations.first().cloned(),
                arguments: arguments.clone(),
                arguments_decoded: Some(json!(arguments_decoded)),
                results: Vec::new(),
                results_decoded: None,
                is_panic_result: false,
                debugger_execution_trace_step_index: debugger_execution_trace.len(),
                nested_calls_ids: Vec::new(),
            };

            // Add the nested call and set it as the current node
            tree.enter_nested_call(call_entry);
        } else if new_fp < prev_fp {
            // If the FP register decreases, that means we have exited the function call
            let prev_trace_entry = &vm_trace[i - 1];
            let prev_sierra_index = mappings.get_first_sierra_index_at_pc(&prev_trace_entry.pc);

            // Get the results of the function call from which we have just exited
            (results, results_decoded) = match prev_sierra_index {
                Some(sierra_index) => mappings.get_results_at_trace_step(
                    relocated_memory,
                    sierra_index.clone(),
                    prev_trace_entry,
                ),
                None => (Vec::new(), Vec::new()),
            };

            tree.set_results_to_current_node(results.clone(), results_decoded.clone());
            // Return to the parent function call
            tree.move_to_parent();
        } else {
            let current_function = tree.get_current_node_data();
            if let Some(cairo_location) = cairo_locations.first() {
                if current_function.cairo_location.is_none() {
                    tree.set_cairo_location_to_current_node(cairo_location.clone());
                }
            }
        }
        prev_fp = trace_entry.fp;
        let parent_function_id = get_internal_function_call_id(&parent_contract_call_id, prev_fp);

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
                        debugger_execution_trace.push(DebuggerExecutionTraceEntry::WithLocation(
                            DebuggerExecutionTraceEntryWithLocation {
                                sierra_index,
                                results: Some(json!(results_decoded.clone())),
                                arguments: Some(json!(arguments_decoded.clone())),
                                location_index,
                                function_id: Some(parent_function_id.clone()),
                            },
                        ));
                        // If current step has the same Cairo location as the last step with Cairo location
                    } else if cairo_location == &prev_cairo_location.unwrap()
                        || cairo_locations == prev_cairo_locations
                    {
                        // If there are arguments or results
                        if results.len() > 0 || arguments.len() > 0 {
                            // Find the last step with Cairo location (not WithContractCall) and update it with the current results and arguments
                            if let Some(DebuggerExecutionTraceEntry::WithLocation(
                                last_with_location,
                            )) = debugger_execution_trace.iter_mut().rev().find(|entry| {
                                matches!(entry, DebuggerExecutionTraceEntry::WithLocation(_))
                            }) {
                                last_with_location.function_id = Some(parent_function_id.clone());
                                last_with_location.results = Some(json!(results_decoded.clone()));
                                last_with_location.arguments =
                                    Some(json!(arguments_decoded.clone()));
                            }
                        }
                    // If current step has a different Cairo location than the last step with Cairo location
                    } else {
                        debugger_execution_trace.push(DebuggerExecutionTraceEntry::WithLocation(
                            DebuggerExecutionTraceEntryWithLocation {
                                sierra_index,
                                results: Some(json!(results_decoded.clone())),
                                arguments: Some(json!(arguments_decoded.clone())),
                                location_index,
                                function_id: Some(parent_function_id.clone()),
                            },
                        ));
                    }

                    prev_cairo_location = Some(cairo_location.clone());
                }
                prev_cairo_locations = cairo_locations.clone();
            }
        }

        let system_call = mappings.get_system_call_at_trace_step(relocated_memory, trace_entry);
        match system_call {
            Some(ESysCall::ContractCall(contract)) => {
                let contract_call_id =
                    get_contract_call_id(Some(parent_contract_call_id), contract_call_index);
                tree.push_contract_call_to_calls_order(&contract_call_id);
                debugger_execution_trace.push(DebuggerExecutionTraceEntry::WithContractCall(
                    DebuggerExecutionTraceEntryWithContractCall {
                        contract_call: contract,
                        contract_call_id,
                    },
                ));
                contract_call_index += 1;
            }
            None => {}
        }
    }

    tree.set_deepest_panic_result();

    Ok((tree.get_root_serializable(), debugger_execution_trace))
}

pub type DecodedData = Vec<Value>;

#[derive(Debug, Clone, Serialize)]
pub struct InternalFnCallTraceEntry {
    pub id: String,
    pub fn_name: Option<String>,
    pub fp: usize,
    pub results: Vec<InternalFnCallIO>,
    pub results_decoded: Option<Value>,
    pub arguments: Vec<InternalFnCallIO>,
    pub arguments_decoded: Option<Value>,
    pub cairo_location: Option<CodeLocation>,
    pub is_panic_result: bool,
    pub debugger_execution_trace_step_index: usize,
    pub nested_calls_ids: Vec<String>,
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
    contract_call_id: String,
}

impl InternalFnCallTraceTree {
    fn new(root_entry: InternalFnCallTraceEntry, contract_call_id: String) -> Self {
        let mut arena = Arena::new();
        let root = arena.new_node(root_entry);
        InternalFnCallTraceTree {
            arena,
            current_node: root,
            root,
            contract_call_id,
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

    fn enter_nested_call(&mut self, entry: InternalFnCallTraceEntry) {
        let contract_call_id = self.contract_call_id.clone();
        if let Some(node) = self.arena.get_mut(self.current_node) {
            let data = node.get_mut();
            data.nested_calls_ids
                .push(get_internal_function_call_id(&contract_call_id, entry.fp));
        }

        let child = self.arena.new_node(entry);
        self.current_node.append(child, &mut self.arena);
        self.current_node = child;
    }

    fn push_contract_call_to_calls_order(&mut self, contract_call_id: &String) {
        if let Some(node) = self.arena.get_mut(self.current_node) {
            let data = node.get_mut();
            data.nested_calls_ids.push(contract_call_id.to_string());
        }
    }

    fn move_to_parent(&mut self) {
        let mut ancestors = self.current_node.ancestors(&self.arena);
        ancestors.next();
        if let Some(parent) = ancestors.next() {
            self.current_node = parent;
        }
    }

    fn set_results_to_current_node(
        &mut self,
        results: Vec<InternalFnCallIO>,
        results_decoded: Vec<Value>,
    ) {
        if let Some(node) = self.arena.get_mut(self.current_node) {
            let data = node.get_mut();
            data.results = results;
            data.results_decoded = Some(json!(results_decoded));
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

    fn set_cairo_location_to_current_node(&mut self, cairo_location: CodeLocation) {
        if let Some(node) = self.arena.get_mut(self.current_node) {
            let data = node.get_mut();
            data.cairo_location = Some(cairo_location);
        }
    }
}
