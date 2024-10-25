pub mod felt252_serde;
pub mod felt252_vec_compression;

use anyhow::anyhow;
use cairo_vm::vm::trace::trace_entry::RelocatedTraceEntry;
use num_bigint::BigUint;
use serde::Serialize;
use starknet::core::types::{BlockId, BlockTag, ContractStorageDiffItem, Felt, StorageEntry};
use starknet_api::core::ChainId;
use starknet_old::core::types as starknet_old_types;
use starknet_providers::jsonrpc::{HttpTransport, JsonRpcClient};
use starknet_selector_decoder::get_selector;
use url::Url;

#[derive(Serialize, Debug, Clone)]
pub struct Datas {
    pub names: String,
    pub types: String,
}

#[derive(Serialize, Debug, Clone)]
pub struct EventAbi {
    pub name: String,
    pub parameters: Vec<Parameter>,
}

#[derive(Serialize, Debug, Clone)]
pub struct Parameter {
    pub name: String,
    pub type_name: String,
}

#[derive(Serialize, Debug, Clone)]
pub struct StructAbi {
    pub name: String,
    pub parameters: Vec<Parameter>,
}

#[derive(Serialize, Debug, Clone)]
pub struct EnumAbi {
    pub name: String,
    pub parameters: Vec<Parameter>,
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
    match chain_id {
        ChainId::Mainnet => {
            Url::parse("https://starknet-mainnet.g.alchemy.com/v2/9J1ION8Owu9eHgZeyWlE9-N0yEepGA58")
                .unwrap()
        }
        ChainId::Sepolia => {
            Url::parse("https://starknet-sepolia.g.alchemy.com/v2/9J1ION8Owu9eHgZeyWlE9-N0yEepGA58")
                .unwrap()
        }
        _ => panic!("Invalid chain id"),
    }
}

pub fn get_voyager_api_url(chain_id: &ChainId) -> &str {
    match chain_id {
        ChainId::Mainnet => "https://api.voyager.online/beta/",
        ChainId::Sepolia => "https://sepolia-api.voyager.online/beta",
        _ => panic!("Invalid chain id"),
    }
}

pub fn extract_chain_id(chain_id: &str) -> anyhow::Result<ChainId> {
    match chain_id {
        "0x534e5f4d41494e" => Ok(ChainId::Mainnet),
        "SN_MAIN" => Ok(ChainId::Mainnet),
        "sn_main" => Ok(ChainId::Mainnet),
        "0x534e5f5345504f4c4941" => Ok(ChainId::Sepolia),
        "SN_SEPOLIA" => Ok(ChainId::Sepolia),
        "sn_sepolia" => Ok(ChainId::Sepolia),
        _ => Err(anyhow!("Invalid chain id")),
    }
}

pub fn chain_id_to_readable_string(chain_id: &ChainId) -> String {
    match chain_id {
        ChainId::Mainnet => String::from("sn_main"),
        ChainId::Sepolia => String::from("sn_sepolia"),
        _ => panic!("Invalid chain id"),
    }
}

pub fn bytes_to_text(bytes: [u8; 32]) -> Result<String, std::str::Utf8Error> {
    let mut text = std::str::from_utf8(&bytes)?.to_string();
    text.retain(|c| c != '\0');
    Ok(text)
}

pub fn felt_vec_to_hex_vec(felt_array: Vec<Felt>) -> Vec<String> {
    let hex_representation = felt_array
        .iter()
        .map(|felt| felt.to_fixed_hex_string())
        .collect::<Vec<String>>();
    hex_representation
}

pub fn decode_felt(felt_array: Vec<Felt>) -> Result<String, std::str::Utf8Error> {
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

pub fn clone_vm_trace(vm_trace: &Vec<RelocatedTraceEntry>) -> Vec<RelocatedTraceEntry> {
    vm_trace
        .iter()
        .map(|trace_entry| RelocatedTraceEntry {
            pc: trace_entry.pc,
            fp: trace_entry.fp,
            ap: trace_entry.ap,
        })
        .collect()
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

pub fn felt_to_field_element(felt: Felt) -> starknet_old_types::FieldElement {
    starknet_old_types::FieldElement::from_bytes_be(&felt.to_bytes_be()).unwrap()
}

pub fn field_element_to_felt(field_element: starknet_old_types::FieldElement) -> Felt {
    Felt::from_bytes_be(&field_element.to_bytes_be())
}

pub fn vec_field_element_to_vec_felt(
    field_elements: Vec<starknet_old_types::FieldElement>,
) -> Vec<Felt> {
    field_elements
        .into_iter()
        .map(|field_element| field_element_to_felt(field_element))
        .collect()
}

pub fn block_id_to_old_block_id(block_id: BlockId) -> starknet_old_types::BlockId {
    match block_id {
        BlockId::Number(block_number) => starknet_old_types::BlockId::Number(block_number),
        BlockId::Hash(block_hash) => {
            starknet_old_types::BlockId::Hash(felt_to_field_element(block_hash))
        }
        BlockId::Tag(block_tag) => match block_tag {
            BlockTag::Latest => {
                starknet_old_types::BlockId::Tag(starknet_old_types::BlockTag::Latest)
            }
            BlockTag::Pending => {
                starknet_old_types::BlockId::Tag(starknet_old_types::BlockTag::Pending)
            }
        },
    }
}

pub fn old_storage_diffs_to_storage_diffs(
    old_storage_diffs: Vec<starknet_old_types::ContractStorageDiffItem>,
) -> Vec<ContractStorageDiffItem> {
    old_storage_diffs
        .into_iter()
        .map(|old_storage_diff| ContractStorageDiffItem {
            address: field_element_to_felt(old_storage_diff.address),
            storage_entries: old_storage_diff
                .storage_entries
                .into_iter()
                .map(|storage_entry| StorageEntry {
                    key: field_element_to_felt(storage_entry.key),
                    value: field_element_to_felt(storage_entry.value),
                })
                .collect(),
        })
        .collect()
}

pub fn get_name_of_entry_point_selector(entry_point_selector: &Felt) -> Option<String> {
    // additional_info.entry_point_function_selector = Some(entry_point_selector.0.to_string()); TODO

    let entry_point_selector_str = entry_point_selector.to_fixed_hex_string();
    let selector = get_selector(&entry_point_selector_str);
    match selector {
        Some(name) => Some(name.to_string()),
        None => None,
    }
}
