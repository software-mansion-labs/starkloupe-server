use std::default;

use anyhow::anyhow;
use cairo_felt::Felt252;
use cairo_lang_sierra::{
    ids::GenericTypeId,
    program::{GenericArg, TypeDeclaration},
};
use cairo_vm::{
    hint_processor::hint_processor_utils::felt_to_usize, vm::trace::trace_entry::TraceEntry,
};
use cheatnet::runtime_extensions::forge_runtime_extension::cheatcodes::spy_events::Event;
use conversions::IntoConv;
use num_bigint::BigUint;
use serde::Serialize;
use starknet::core::types::FieldElement;
use starknet_api::core::ChainId;
use starknet_api::hash::StarkFelt;
use starknet_providers::jsonrpc::{HttpTransport, JsonRpcClient};
use url::Url;

#[derive(Serialize, Debug, Clone)]
pub struct Datas {
    pub names: String,
    pub types: String,
}

#[derive(Serialize, Debug, Clone)]
pub struct EventItems {
    pub name: String,
    pub members: Vec<Datas>,
}

#[derive(Serialize, Debug, Clone)]
pub struct StructItems {
    pub name: String,
    pub members: Vec<Datas>,
}

#[derive(Serialize, Debug, Clone)]
pub struct EnumItems {
    pub variant: Option<String>,
    pub data_type: String,
}

pub const MAIN_CHAIN_ID: &str = "0x534e5f4d41494e";
pub const SEPOLIA_CHAIN_ID: &str = "0x534e5f5345504f4c4941";

pub const STRK_FEE_TOKEN_ADDRESS: &str =
    "0x04718f5a0fc34cc1af16a1cdee98ffb20c31f5cd61d6ab07201858f4287c938d";
pub const ETH_FEE_TOKEN_ADDRESS: &str =
    "0x049d36570d4e46f48e99674bd3fcc84644ddd6b96f7c741b1562b82f9e004dc7";

pub fn create_rpc_client(chain_id: &ChainId) -> JsonRpcClient<HttpTransport> {
    JsonRpcClient::new(HttpTransport::new(rpc_url(chain_id)))
}

pub fn create_rpc_client_from_url(rpc_url: Url) -> JsonRpcClient<HttpTransport> {
    JsonRpcClient::new(HttpTransport::new(rpc_url))
}

pub fn rpc_url(chain_id: &ChainId) -> Url {
    match chain_id.0.as_str() {
        MAIN_CHAIN_ID => {
            Url::parse("https://starknet-mainnet.g.alchemy.com/v2/9J1ION8Owu9eHgZeyWlE9-N0yEepGA58")
                .unwrap()
        }
        SEPOLIA_CHAIN_ID => {
            Url::parse("https://starknet-sepolia.g.alchemy.com/v2/9J1ION8Owu9eHgZeyWlE9-N0yEepGA58")
                .unwrap()
        }
        _ => panic!("Invalid chain id"),
    }
}

pub fn get_voyager_api_url(chain_id: &ChainId) -> &str {
    match chain_id.0.as_str() {
        MAIN_CHAIN_ID => "https://api.voyager.online/beta/",
        SEPOLIA_CHAIN_ID => "https://sepolia-api.voyager.online/beta ",
        _ => panic!("Invalid chain id"),
    }
}

pub fn extract_chain_id(chain_id: &str) -> anyhow::Result<ChainId> {
    let main = ChainId(MAIN_CHAIN_ID.to_string());
    let sepolia = ChainId(SEPOLIA_CHAIN_ID.to_string());
    match chain_id {
        "0x534e5f4d41494e" => Ok(main),
        "SN_MAIN" => Ok(main),
        "sn_main" => Ok(main),
        "0x534e5f5345504f4c4941" => Ok(sepolia),
        "SN_SEPOLIA" => Ok(sepolia),
        "sn_sepolia" => Ok(sepolia),
        _ => Err(anyhow!("Invalid chain id")),
    }
}

pub fn chain_id_to_readable_string(chain_id: &ChainId) -> String {
    match chain_id.0.as_str() {
        MAIN_CHAIN_ID => String::from("sn_main"),
        SEPOLIA_CHAIN_ID => String::from("sn_sepolia"),
        _ => panic!("Invalid chain id"),
    }
}

pub fn bytes_to_text(bytes: [u8; 32]) -> Result<String, std::str::Utf8Error> {
    let mut text = std::str::from_utf8(&bytes)?.to_string();
    text.retain(|c| c != '\0');
    Ok(text)
}

pub fn felt252_to_hex(felt_array: Vec<Felt252>) -> Result<Vec<String>, std::str::Utf8Error> {
    let hex_representation = felt_array
        .iter()
        .map(|felt| format!("0x{:0>64}", felt.to_str_radix(16)))
        .collect::<Vec<String>>();

    Ok(hex_representation)
}

pub fn decode_felt252(felt_array: Vec<Felt252>) -> Result<String, std::str::Utf8Error> {
    //convert do decimal string representation
    let decimal_arrays = felt_array
        .iter()
        .map(|felt| felt.to_string())
        .collect::<Vec<String>>();
    let decimal_string = decimal_arrays.join(", ");
    //convert to hex representation
    let hex_representation = BigUint::parse_bytes(decimal_string.as_bytes(), 10)
        .expect("Failed to parse BigUint")
        .to_str_radix(16);
    //conver it to bytes
    let bytes: Vec<u8> = hex_representation
        .as_bytes()
        .chunks(2)
        .map(|chunk| u8::from_str_radix(std::str::from_utf8(chunk).unwrap(), 16).unwrap())
        .collect();
    //get human readable text
    let text = String::from_utf8_lossy(&bytes);
    Ok(text.to_string())
}

pub fn felt_vec_to_event_vec(felts: &[Felt252]) -> Vec<Event> {
    let mut events = vec![];
    let mut i = 0;
    while i < felts.len() {
        let from = felts[i].clone().into_();
        let keys_length = &felts[i + 1];
        let keys = &felts[i + 2..i + 2 + felt_to_usize(keys_length).unwrap()];
        let data_length = &felts[i + 2 + felt_to_usize(keys_length).unwrap()];
        let data = &felts[i + 2 + felt_to_usize(keys_length).unwrap() + 1
            ..i + 2
                + felt_to_usize(keys_length).unwrap()
                + 1
                + felt_to_usize(data_length).unwrap()];

        events.push(Event {
            from,
            keys: Vec::from(keys),
            data: Vec::from(data),
        });

        i = i + 2 + felt_to_usize(keys_length).unwrap() + 1 + felt_to_usize(data_length).unwrap();
    }

    events
}

pub fn starkfelt_vec_to_fieldelement_vec(calldata: &[StarkFelt]) -> Vec<FieldElement> {
    calldata
        .iter()
        .map(|starkfelt| FieldElement::from_bytes_be(starkfelt.bytes()).unwrap())
        .collect()
}

pub fn clone_vm_trace(vm_trace: &Vec<TraceEntry>) -> Vec<TraceEntry> {
    vm_trace
        .iter()
        .map(|trace_entry| TraceEntry {
            pc: trace_entry.pc,
            fp: trace_entry.fp,
            ap: trace_entry.ap,
        })
        .collect()
}

pub fn pad_field_element_to_hex_string_length66(field_element: FieldElement) -> String {
    let hex_string = hex::encode(field_element.to_bytes_be());
    format!("0x{:0>64}", hex_string)
}

pub fn get_contract_call_id(
    parent_contract_call_id: Option<&str>,
    contract_call_index: usize,
) -> String {
    match parent_contract_call_id {
        Some(parent_contract_call_id) => {
            format!("{}-{}", parent_contract_call_id, contract_call_index)
        }
        None => contract_call_index.to_string(),
    }
}

pub fn get_internal_function_call_id(contract_call_id: &str, fp: usize) -> String {
    format!("{}-fp-{}", contract_call_id, fp)
}

/// Pads a given hex string (starting with "0x") to 66 characters, including "0x".
/// This function adds zeros after "0x" to achieve the desired length.
pub fn pad_hex_string_to_66(hex_str: &str) -> String {
    if !hex_str.starts_with("0x") {
        panic!("Hex string must start with '0x'");
    }
    format!("0x{:0>64}", &hex_str[2..])
}

pub fn build_data_items_from_type_declaration(
    type_declaration: &Option<TypeDeclaration>,
    type_declarations: &[TypeDeclaration],
) -> (Option<Vec<EnumItems>>, Option<Vec<StructItems>>) {
    let type_declaration = match type_declaration {
        Some(decl) => decl,
        None => return (None, None),
    };

    let mut enum_items: Vec<EnumItems> = Vec::new();
    let mut struct_items: Vec<StructItems> = Vec::new();
    let mut members: Vec<Datas> = Vec::new();

    let struct_name = type_declaration
        .id
        .debug_name
        .as_deref()
        .unwrap_or("")
        .to_string();

    for arg in &type_declaration.long_id.generic_args {
        if let GenericArg::Type(concrete_type_id) = arg {
            if let Some(nested_type_declaration) = type_declarations
                .iter()
                .find(|type_decl| type_decl.id.id == concrete_type_id.id)
                .cloned()
            {
                let nested_type_name = nested_type_declaration
                    .id
                    .debug_name
                    .as_deref()
                    .unwrap_or("")
                    .to_string();

                // Handle Enum types only if the main type is an Enum
                if type_declaration.long_id.generic_id == GenericTypeId::from_string("Enum") {
                    enum_items.push(EnumItems {
                        variant: None,
                        data_type: nested_type_name.clone(),
                    });
                }

                // Handle Struct types for both Enum and Struct cases
                members.push(Datas {
                    names: "".to_string(),
                    types: nested_type_name.clone(),
                });

                let (_, nested_struct_items) = build_data_items_from_type_declaration(
                    &Some(nested_type_declaration),
                    type_declarations,
                );

                if let Some(nested_struct_items) = nested_struct_items {
                    struct_items.extend(nested_struct_items);
                }
            }
        }
    }

    if !members.is_empty() {
        struct_items.push(StructItems {
            name: struct_name,
            members,
        });
    }

    (Some(enum_items), Some(struct_items))
}
