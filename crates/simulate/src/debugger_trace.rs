use internal_tracing::{
    call_trace::{DebuggerTraceEntry, DebuggerTraceEntryWithContractCall},
    function_calls_map::FunctionCallsMap,
};
use std::collections::HashMap;

use crate::contract_calls_map::ContractCallsMap;

pub struct DebuggerTraceBuilder {
    contract_calls_debugger_trace_step_index_map: HashMap<u32, usize>,
    trace: Vec<DebuggerTraceEntry>,
}

impl DebuggerTraceBuilder {
    pub fn build(
        contract_call_id: &u32,
        function_calls_map: &mut FunctionCallsMap,
        contract_calls_map: &mut ContractCallsMap,
    ) -> Vec<DebuggerTraceEntry> {
        let mut builder = DebuggerTraceBuilder {
            contract_calls_debugger_trace_step_index_map: HashMap::new(),
            trace: Vec::new(),
        };

        builder.merge_debugger_trace(contract_call_id, function_calls_map, contract_calls_map);

        for (contract_call_id, debugger_trace_step_index) in
            &builder.contract_calls_debugger_trace_step_index_map
        {
            let contract_call = contract_calls_map.0.get_mut(contract_call_id).unwrap();
            contract_call.debugger_trace_step_index = Some(*debugger_trace_step_index);
        }

        for (_function_call_id, function_call) in function_calls_map.0.iter_mut() {
            if function_call.debugger_trace_step_index.is_none() {
                let contract_call = contract_calls_map
                    .0
                    .get(&function_call.contract_call_id)
                    .unwrap();
                function_call.debugger_trace_step_index = contract_call.debugger_trace_step_index;
            }
        }

        builder.trace
    }

    fn merge_debugger_trace(
        &mut self,
        contract_call_id: &u32,
        function_calls_map: &mut FunctionCallsMap,
        contract_calls_map: &ContractCallsMap,
    ) {
        let mut no_trace_reason: Option<String> = None;

        if let Some(contract_call) = contract_calls_map.0.get(contract_call_id) {
            if let Some(call_debugger_data) = &contract_call.call_debugger_data {
                if !call_debugger_data.execution_trace.is_empty() {
                    for step in call_debugger_data.execution_trace.iter() {
                        match step {
                            DebuggerTraceEntry::WithLocation(with_location) => {
                                self.trace
                                    .push(DebuggerTraceEntry::WithLocation(with_location.clone()));

                                let function_call = function_calls_map
                                    .0
                                    .get_mut(&with_location.function_call_id)
                                    .unwrap();

                                let debugger_trace_step_index = self.trace.len() - 1;

                                if function_call.debugger_trace_step_index.is_none() {
                                    function_call.debugger_trace_step_index =
                                        Some(debugger_trace_step_index);
                                }

                                if !self
                                    .contract_calls_debugger_trace_step_index_map
                                    .contains_key(contract_call_id)
                                {
                                    self.contract_calls_debugger_trace_step_index_map
                                        .insert(*contract_call_id, debugger_trace_step_index);
                                }
                            }
                            DebuggerTraceEntry::WithContractCall(with_contract_call) => {
                                self.merge_debugger_trace(
                                    &with_contract_call.contract_call_id,
                                    function_calls_map,
                                    contract_calls_map,
                                );
                            }
                        }
                    }
                } else {
                    no_trace_reason = Some("Execution trace is empty".to_string());
                }
            } else {
                no_trace_reason = Some("No call debugger data available".to_string());
            }
        } else {
            no_trace_reason = Some("Contract call not found".to_string());
        }

        if let Some(reason) = no_trace_reason {
            self.trace.push(DebuggerTraceEntry::WithContractCall(
                DebuggerTraceEntryWithContractCall {
                    contract_call_id: *contract_call_id,
                    reason: Some(reason),
                },
            ));

            for child_contract_call_id in contract_calls_map
                .0
                .get(contract_call_id)
                .unwrap()
                .children_call_ids
                .iter()
            {
                self.merge_debugger_trace(
                    child_contract_call_id,
                    function_calls_map,
                    contract_calls_map,
                );
            }
        }
    }
}
