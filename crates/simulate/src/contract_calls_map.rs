use crate::contract_call::ContractCall;
use crate::FlameChartNode;
use cheatnet::runtime_extensions::forge_runtime_extension::cheatcodes::spy_events::Event;
use cheatnet::state::CallTrace;
use cheatnet::state::CallTraceNode;
use cheatnet::state::CheatnetState;
use serde::Serialize;
use starknet_api::abi::abi_utils::selector_from_name;
use starknet_api::transaction::constants;
use std::cell::Ref;
use std::collections::HashMap;

#[derive(Debug, Serialize)]
pub struct ContractCallsMap(pub HashMap<u32, ContractCall>);

impl ContractCallsMap {
    pub fn new() -> Self {
        ContractCallsMap(HashMap::new())
    }

    pub fn collect_all_class_hashes(&self) -> Vec<String> {
        let mut class_hashes = Vec::new();
        for call in self.0.values() {
            if let Some(class_hash) = call.entry_point.class_hash {
                class_hashes.push(class_hash.0.to_fixed_hex_string());
            }
        }
        class_hashes
    }
}

pub struct ContractCallsMapBuilder {
    pub contract_calls_map: ContractCallsMap,
    pub next_call_id: u32,
    pub deepest_failed_contract_call_id: Option<u32>,
    pub deepest_failed_nesting_level: u32,
    pub cheatnet_state_detected_events: Vec<Event>,
}

impl ContractCallsMapBuilder {
    pub fn new_from_cheatnet_state(
        cheatnet_state: CheatnetState,
        contract_flamechart: &mut Vec<FlameChartNode>,
    ) -> Self {
        let mut contract_call_tree_builder = Self {
            contract_calls_map: ContractCallsMap::new(),
            next_call_id: 1,
            deepest_failed_contract_call_id: None,
            deepest_failed_nesting_level: 0,
            cheatnet_state_detected_events: cheatnet_state.detected_events.clone(),
        };

        let call_trace_ref = cheatnet_state
            .trace_data
            .current_call_stack
            .borrow_full_trace();

        let new_contract_call_id = contract_call_tree_builder.next_call_id;
        let root_contract_call = ContractCall::from_cheatnet_state_calltrace(
            &call_trace_ref,
            new_contract_call_id,
            0,
            0,
            true, // Hide root contract call
        );
        contract_call_tree_builder.next_call_id += 1;
        contract_call_tree_builder
            .contract_calls_map
            .0
            .insert(root_contract_call.call_id, root_contract_call);

        contract_call_tree_builder.traverse_cheatnet_state_calltrace(
            new_contract_call_id,
            call_trace_ref,
            contract_flamechart,
            0,
        );

        contract_call_tree_builder
    }

    fn traverse_cheatnet_state_calltrace(
        &mut self,
        current_call_id: u32,
        call_trace_ref: Ref<CallTrace>,
        contract_flamechart: &mut Vec<FlameChartNode>,
        nesting_level: u32,
    ) {
        for nested_call in &call_trace_ref.nested_calls {
            match nested_call {
                CallTraceNode::EntryPointCall(call_trace) => {
                    let new_contract_call_id = self.next_call_id;
                    let is_fee_transfer = call_trace.borrow().entry_point.entry_point_selector
                        == selector_from_name(constants::TRANSFER_ENTRY_POINT_NAME);
                    let contract_call = ContractCall::from_cheatnet_state_calltrace(
                        &call_trace.borrow(),
                        new_contract_call_id,
                        current_call_id,
                        nesting_level + 1,
                        is_fee_transfer,
                    );

                    if contract_call.is_failed {
                        if self.deepest_failed_contract_call_id.is_some() {
                            if nesting_level + 1 > self.deepest_failed_nesting_level {
                                self.deepest_failed_contract_call_id = Some(new_contract_call_id);
                            }
                        } else {
                            self.deepest_failed_contract_call_id = Some(new_contract_call_id);
                            self.deepest_failed_nesting_level = nesting_level + 1;
                        }
                    }

                    let is_hidden = contract_call.is_hidden;

                    let mut flamechart_node = FlameChartNode {
                        call_id: contract_call.call_id,
                        raw_value: contract_call.sierra_gas,
                        value: 0.0,
                        name: None,
                        children: Vec::new(),
                    };

                    self.next_call_id += 1;
                    self.contract_calls_map
                        .0
                        .insert(new_contract_call_id, contract_call);
                    self.contract_calls_map
                        .0
                        .get_mut(&current_call_id)
                        .unwrap()
                        .children_call_ids
                        .push(new_contract_call_id);

                    if !is_hidden {
                        self.traverse_cheatnet_state_calltrace(
                            new_contract_call_id,
                            call_trace.borrow(),
                            &mut flamechart_node.children,
                            nesting_level + 1,
                        );

                        contract_flamechart.push(flamechart_node);
                    } else {
                        self.traverse_cheatnet_state_calltrace(
                            new_contract_call_id,
                            call_trace.borrow(),
                            contract_flamechart,
                            nesting_level + 1,
                        );
                    }
                }
                CallTraceNode::DeployWithoutConstructor => {
                    // TODO: explore
                }
            }
        }
    }
}
