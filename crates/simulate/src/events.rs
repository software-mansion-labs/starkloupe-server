use crate::ContractCallsMap;
use blockifier::abi::abi_utils::selector_from_name;
use data_decoder::calldata_decoder::decode_calldata;
use data_decoder::create_decoded_value;
use data_decoder::{DecodedValue, DecodedValueType};
use serde::Serialize;

use cheatnet::runtime_extensions::forge_runtime_extension::cheatcodes::spy_events::Event;
use starknet_selector_decoder::get_selector;
use std::borrow::Cow;
use std::collections::HashMap;
use walnut_shared::field_element_to_felt;
use walnut_shared::EnumAbi;
use walnut_shared::EventAbi;
use walnut_shared::StructAbi;

#[derive(Debug, Serialize, Clone)]
pub struct EmittedEvent {
    pub contract_call_id: u32,
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
        strkgate_event: Option<starknet_old::core::types::Event>,
    ) -> Vec<EmittedEvent> {
        let mut events = Vec::new();

        // The StarkNet transaction emits this `Transfer` event from the StarkGate ETH token contract.
        // This event is present inside the transaction receipt but is not found in the Foundry-emitted
        // events array.
        // To maintain consistency with blockchain explorers, we need to manually append this event
        // to the vector of all events.
        if let Some(event) = strkgate_event {
            let strkgate_event_decoded = Self::convert_event_to_emitted(&event);
            events.push(strkgate_event_decoded);
        }

        let mut storage_address_to_call_id = HashMap::new();
        for call in contract_calls_map.0.values() {
            storage_address_to_call_id.insert(call.entry_point.storage_address, call.call_id);
        }

        for cheatnet_state_event in cheatnet_state_detected_events.iter().rev() {
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
                            contract_call_id: *contract_call_id,
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

    fn convert_event_to_emitted(event: &starknet_old::core::types::Event) -> EmittedEvent {
        let selector_felt = field_element_to_felt(event.keys[0]); // Konverzija FieldElement u Felt
        let selector_str = selector_felt.to_fixed_hex_string(); // Pretvaranje u hex string
        let event_name = get_selector(&selector_str)
            .unwrap_or("Transfer")
            .to_string();

        let datas = vec![
            create_decoded_value(
                Some("from"),
                "ContractAddress",
                DecodedValueType::Single(field_element_to_felt(event.data[0])),
            ),
            create_decoded_value(
                Some("to"),
                "ContractAddress",
                DecodedValueType::Single(field_element_to_felt(event.data[1])),
            ),
            create_decoded_value(
                Some("amount"),
                "u256",
                DecodedValueType::Struct({
                    let mut amount_map = HashMap::new();
                    amount_map.insert(
                        0,
                        create_decoded_value(
                            Some("low"),
                            "u128",
                            DecodedValueType::Single(field_element_to_felt(event.data[2])),
                        ),
                    );
                    amount_map.insert(
                        1,
                        create_decoded_value(
                            Some("high"),
                            "u128",
                            DecodedValueType::Single(field_element_to_felt(event.data[3])),
                        ),
                    );
                    amount_map
                }),
            ),
        ];

        EmittedEvent {
            contract_call_id: 0, // Ako nema, možeš koristiti `None` ako je opcioni tip
            name: event_name,
            selector: selector_str,
            datas: Some(datas),
        }
    }
}
