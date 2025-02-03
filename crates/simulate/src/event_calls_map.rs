use crate::{event_call::EventCall, ContractCallsMap};
use blockifier::abi::abi_utils::selector_from_name;

use cheatnet::runtime_extensions::forge_runtime_extension::cheatcodes::spy_events::Event;
use serde::Serialize;
use std::collections::HashMap;
use walnut_shared::felt_vec_to_hex_vec;
use walnut_shared::EventAbi;

#[derive(Debug, Serialize, Default)]
pub struct EventCallsMap(pub HashMap<u32, EventCall>);

impl EventCallsMap {
    pub fn create_event_calls_map(
        contract_calls_map: &mut ContractCallsMap,
        next_call_id: &mut u32,
        event_abis: &[EventAbi],
        cheatnet_state_detected_events: &[Event],
    ) -> Self {
        let mut event_calls_map = EventCallsMap::default();
        let mut storage_address_to_call_id = HashMap::new();
        for call in contract_calls_map.0.values() {
            storage_address_to_call_id.insert(call.entry_point.storage_address, call.call_id);
        }

        for cheatnet_state_event in cheatnet_state_detected_events {
            let event_selector = cheatnet_state_event.keys[0];

            if let Some(contract_call_id) =
                storage_address_to_call_id.get(&cheatnet_state_event.from)
            {
                if let Some(contract_call) = contract_calls_map.0.get_mut(contract_call_id) {
                    if let Some(event_abi) = event_abis
                        .iter()
                        .find(|abi| selector_from_name(&abi.name).0 == event_selector)
                    {
                        let new_event_call_id = *next_call_id;

                        let event = EventCall {
                            call_id: new_event_call_id,
                            contract_call_id: *contract_call_id,
                            name: event_abi.name.clone(),
                            keys: felt_vec_to_hex_vec(cheatnet_state_event.keys.to_vec()),
                            parameters: event_abi.parameters.clone(),
                            data: felt_vec_to_hex_vec(cheatnet_state_event.data.to_vec()),
                            debugger_trace_step_index: None,
                            code_location: None,
                            is_hidden: false,
                        };

                        contract_call.event_call_ids.push(new_event_call_id);
                        *next_call_id += 1;
                        event_calls_map.0.insert(new_event_call_id, event);
                    }
                }
            }
        }
        event_calls_map
    }
}
