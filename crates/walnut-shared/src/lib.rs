use cairo_felt::Felt252;
use cairo_vm::{
    hint_processor::hint_processor_utils::felt_to_usize, vm::trace::trace_entry::TraceEntry,
};
use cheatnet::runtime_extensions::forge_runtime_extension::cheatcodes::spy_events::Event;
use conversions::IntoConv;
use num_bigint::BigUint;
use serde::Serialize;
use starknet::core::types::FieldElement;
use starknet_api::core::ChainId;
use starknet_providers::jsonrpc::{HttpTransport, JsonRpcClient};
use url::Url;

#[derive(Serialize, Debug, Clone)]
pub struct EventItems {
    pub name: String,
    pub members: Vec<Datas>,
}

#[derive(Serialize, Debug, Clone)]
pub struct Datas {
    pub names: String,
    pub types: String,
}

#[derive(Serialize, Debug, Clone)]
pub struct StructItems {
    pub name: String,
    pub members: Vec<Datas>,
}
const MAIN_CHAIN_ID: &str = "0x534e5f4d41494e";
const SEPOLIA_CHAIN_ID: &str = "0x534e5f5345504f4c4941";

pub fn create_rpc_client(chain_id: &ChainId) -> JsonRpcClient<HttpTransport> {
    JsonRpcClient::new(HttpTransport::new(Url::parse(rpc_url(chain_id)).unwrap()))
}

pub fn rpc_url(chain_id: &ChainId) -> &str {
    match chain_id.0.as_str() {
        MAIN_CHAIN_ID => {
            "https://starknet-mainnet.g.alchemy.com/v2/9J1ION8Owu9eHgZeyWlE9-N0yEepGA58"
        }
        SEPOLIA_CHAIN_ID => {
            "https://starknet-sepolia.g.alchemy.com/v2/9J1ION8Owu9eHgZeyWlE9-N0yEepGA58"
        }
        _ => panic!("Invalid chain id"),
    }
}

pub fn voyager_api_url(chain_id: &ChainId) -> &str {
    match chain_id.0.as_str() {
        MAIN_CHAIN_ID => "https://api.voyager.online/beta/",
        SEPOLIA_CHAIN_ID => "https://sepolia-api.voyager.online/beta ",
        _ => panic!("Invalid chain id"),
    }
}

pub fn extract_chain_id(chain_id: &str) -> ChainId {
    let main = ChainId(MAIN_CHAIN_ID.to_string());
    let sepolia = ChainId(SEPOLIA_CHAIN_ID.to_string());
    match chain_id {
        "0x534e5f4d41494e" => main,
        "SN_MAIN" => main,
        "sn_main" => main,
        "0x534e5f5345504f4c4941" => sepolia,
        "SN_SEPOLIA" => sepolia,
        "sn_sepolia" => sepolia,
        _ => panic!("Invalid chain id"),
    }
}

pub fn chain_id_to_readable_string(chain_id: ChainId) -> String {
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
    let mut hex_string = hex::encode(field_element.to_bytes_be());
    while hex_string.len() < 64 {
        hex_string.insert_str(0, "0");
    }
    format!("0x{}", hex_string)
}
