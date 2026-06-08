use crate::contract_names::ContractName;
use crate::ContractCall;
use crate::ContractCallsMap;
use cheatnet::runtime_extensions::forge_runtime_extension::cheatcodes::spy_events::Event as CheatnetEvent;
use data_decoder::calldata_decoder::decode_calldata;
use data_decoder::create_decoded_value_by_type;
use data_decoder::{DecodedValue, DecodedValueType};
use serde::Serialize;
use starknet_api::abi::abi_utils::selector_from_name;
use starknet_api::core::ContractAddress;
use starknet_rust::core::types::Event as StarknetEvent;
use starknet_selector_decoder::get_selector;
use std::borrow::Cow;
use std::collections::HashMap;
use tracing::error;
use walnut_shared::abi::{Enum, Event, EventData, EventKind, Struct};

#[derive(Debug, Serialize, Clone)]
pub struct EmittedEvent {
    pub contract_call_id: u32,
    pub contract_address: Option<ContractAddress>,
    pub contract_name: String,
    pub name: String,
    pub selector: String,
    pub datas: Option<Vec<DecodedValue>>,
}

impl EmittedEvent {
    pub fn create_emitted_events_list(
        contract_calls_map: &mut ContractCallsMap,
        event_abis: &[Event],
        struct_abis: &[Struct],
        enum_abis: &[Enum],
        cheatnet_state_detected_events: &[CheatnetEvent],
        strkgate_emitted_event: Option<EmittedEvent>,
    ) -> Vec<EmittedEvent> {
        fn get_contract_name(contract_call: &ContractCall) -> String {
            let mut contract_name: Option<String> = None;

            if let Some(ref name) = contract_call.contract_name {
                contract_name = Some(name.clone());
            } else if let (Some(ref token_name), Some(ref token_symbol)) = (
                &contract_call.erc20_token_name,
                &contract_call.erc20_token_symbol,
            ) {
                contract_name = Some(format!("{} ({})", token_name, token_symbol));
            } else if let Some(ref entry_point) = contract_call.entry_point_interface_name {
                contract_name = entry_point.split("::").last().map(String::from);
            }

            contract_name.unwrap_or_else(|| {
                contract_call
                    .entry_point
                    .storage_address
                    .to_fixed_hex_string()
            })
        }

        let mut events = Vec::new();

        if let Some(strkgate_emitted_event) = strkgate_emitted_event {
            events.push(strkgate_emitted_event);
        }

        let mut storage_address_to_call_id = HashMap::new();
        for call in contract_calls_map.0.values() {
            storage_address_to_call_id.insert(call.entry_point.storage_address, call.call_id);
        }

        for cheatnet_state_event in cheatnet_state_detected_events.iter().rev() {
            let event_selector = cheatnet_state_event.keys[0];
            let contract_address = cheatnet_state_event.from;
            if let Some(contract_call_id) =
                storage_address_to_call_id.get(&cheatnet_state_event.from)
            {
                if let Some(contract_call) = contract_calls_map.0.get_mut(contract_call_id) {
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

                        let (names, types): (Vec<Cow<str>>, Vec<Cow<str>>) = match &event_abi.data {
                            EventData::Cairo2 { kind } => match kind {
                                EventKind::Struct { members }
                                | EventKind::Enum { variants: members } => members
                                    .iter()
                                    .map(|param| {
                                        (
                                            Cow::Borrowed(param.name.as_str()),
                                            Cow::Borrowed(param.ty.as_str()),
                                        )
                                    })
                                    .unzip(),
                            },
                            EventData::Cairo1 { inputs } => inputs
                                .iter()
                                .map(|param| {
                                    (
                                        Cow::Borrowed(param.name.as_str()),
                                        Cow::Borrowed(param.ty.as_str()),
                                    )
                                })
                                .unzip(),
                        };

                        let decoded_event_data =
                            decode_calldata(&keys.to_vec(), &types, &names, struct_abis, enum_abis);

                        let event = EmittedEvent {
                            contract_call_id: *contract_call_id,
                            contract_address: Some(contract_address),
                            contract_name: get_contract_name(contract_call),
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

    pub fn convert_event_to_emitted_event(
        event: &StarknetEvent,
        contract_name: &Option<ContractName>,
    ) -> Option<EmittedEvent> {
        fn decode_event_data(event: &StarknetEvent) -> Vec<DecodedValue> {
            let low = event.data[2].to_biguint();
            let high = event.data[3].to_biguint();
            let u256_value = (high << 128) | low;

            vec![
                create_decoded_value_by_type(
                    Some("from"),
                    "ContractAddress",
                    DecodedValueType::Single(event.data[0]),
                ),
                create_decoded_value_by_type(
                    Some("to"),
                    "ContractAddress",
                    DecodedValueType::Single(event.data[1]),
                ),
                create_decoded_value_by_type(
                    Some("amount"),
                    "u256",
                    DecodedValueType::BigUint(u256_value),
                ),
            ]
        }

        let selector_str = event.keys[0].to_fixed_hex_string();
        let event_name = get_selector(&selector_str);

        match (event_name, contract_name) {
            (Some(event_name), Some(contract_name)) => {
                if let Some(contract_name) = &contract_name.name {
                    // Only process the event if the contract name is "StarkGate" and event is "Transfer"
                    if event_name == "Transfer"
                        && (contract_name == "StarkGate: ETH Token"
                            || contract_name == "StarkGate: STRK Token")
                        && event.data.len() == 4
                    {
                        let contract_address: Option<ContractAddress> =
                            match ContractAddress::try_from(event.from_address) {
                                Ok(addr) => Some(addr),
                                Err(e) => {
                                    error!("Failed to convert contract address: {}", e);
                                    None
                                }
                            };

                        let datas = decode_event_data(event);
                        Some(EmittedEvent {
                            contract_call_id: 0,
                            contract_address,
                            contract_name: contract_name.to_string(),
                            name: event_name.to_string(),
                            selector: selector_str,
                            datas: Some(datas),
                        })
                    } else {
                        None
                    }
                } else {
                    None
                }
            }
            _ => None,
        }
    }
}
