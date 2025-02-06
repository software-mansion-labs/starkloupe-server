use crate::ContractCallsMap;
use blockifier::abi::abi_utils::selector_from_name;
use data_decoder::calldata_decoder::decode_calldata;
use data_decoder::create_decoded_value;
use data_decoder::{DecodedValue, DecodedValueType};
use serde::Serialize;

use cheatnet::runtime_extensions::forge_runtime_extension::cheatcodes::spy_events::Event;
use std::borrow::Cow;
use std::collections::HashMap;
use walnut_shared::EnumAbi;
use walnut_shared::EventAbi;
use walnut_shared::StructAbi;

#[derive(Debug, Serialize, Clone)]
pub struct EmittedEvent {
    pub contract_call_id: Option<u32>,
    pub name: String,
    pub selector: String,
    pub datas: Option<Vec<DecodedValue>>,
}

impl EmittedEvent {
    pub fn create_emitted_events_list(
        contract_calls_map: &mut ContractCallsMap,
        event_abis: &[EventAbi],
        struct_abis: &[StructAbi],
        enum_abis: &[EnumAbi],
        cheatnet_state_detected_events: &[Event],
    ) -> Vec<EmittedEvent> {
        let mut events = Vec::new();
        let mut storage_address_to_call_id = HashMap::new();
        for call in contract_calls_map.0.values() {
            storage_address_to_call_id.insert(call.entry_point.storage_address, call.call_id);
        }

        for cheatnet_state_event in cheatnet_state_detected_events {
            let event_selector = cheatnet_state_event.keys[0];

            if let Some(contract_call_id) =
                storage_address_to_call_id.get(&cheatnet_state_event.from)
            {
                if let Some(_contract_call) = contract_calls_map.0.get_mut(contract_call_id) {
                    if let Some(event_abi) = event_abis
                        .iter()
                        .find(|abi| selector_from_name(&abi.name).0 == event_selector)
                    {
                        let mut keys = cheatnet_state_event.keys.to_vec();
                        let data = cheatnet_state_event.data.to_vec();

                        keys.extend(data.clone());
                        if !keys.is_empty() {
                            keys.remove(0);
                        }

                        let (names, types): (Vec<Cow<str>>, Vec<Cow<str>>) = event_abi
                            .parameters
                            .iter()
                            .map(|param| {
                                (
                                    Cow::Owned(param.name.clone()),
                                    Cow::Owned(param.type_name.clone()),
                                )
                            })
                            .unzip();

                        let decoded_event_data = decode_calldata(
                            &keys.to_vec(),
                            &types,
                            &names,
                            Some(struct_abis),
                            Some(enum_abis),
                            &mut 0,
                        );

                        let event = EmittedEvent {
                            contract_call_id: Some(*contract_call_id),
                            name: event_abi.name.clone(),
                            selector: event_selector.to_fixed_hex_string(),
                            datas: decoded_event_data,
                        };

                        events.push(event);
                    }
                }
            }
        }
        events
    }
}
