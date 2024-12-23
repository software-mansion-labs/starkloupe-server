pub mod felt252_serde;
pub mod felt252_vec_compression;

use anyhow::{anyhow, Result};
use cairo_vm::vm::trace::trace_entry::RelocatedTraceEntry;
use serde::Serialize;
use starknet::core::types::{
    BlockId, BlockTag, ContractStorageDiffItem, DeclaredClassItem, DeployedContractItem,
    ExecutionResult, Felt, StorageEntry,
};
use starknet_api::{
    core::ChainId,
    transaction::{Resource, ResourceBounds, ResourceBoundsMapping},
};
use starknet_old::core::types::{self as starknet_old_types};
use starknet_providers::jsonrpc::{HttpTransport, JsonRpcClient};
use starknet_selector_decoder::get_selector;
use std::collections::BTreeMap;
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

pub fn get_voyager_api_url(chain_id: &ChainId) -> Option<&str> {
    match chain_id {
        ChainId::Mainnet => Some("https://api.voyager.online/beta/"),
        ChainId::Sepolia => Some("https://sepolia-api.voyager.online/beta"),
        _ => None,
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
        ChainId::Other(chain_id) => chain_id.clone(),
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

pub fn felts_to_string(felts: &[Felt]) -> Vec<String> {
    let mut decoded_strings = Vec::new();

    for felt in felts {
        // Convert Felt to BigUint
        let value = felt.to_biguint();

        // Convert BigUint to bytes (big-endian)
        let mut felt_bytes = value.to_bytes_be();

        // Remove leading zeros (optional)
        while let Some(0) = felt_bytes.first() {
            felt_bytes.remove(0);
        }

        // Attempt to decode bytes into a String
        match String::from_utf8(felt_bytes.clone()) {
            Ok(s) => decoded_strings.push(s),
            Err(_) => {
                // Include the raw hexadecimal representation
                let hex_repr = format!("0x{}", hex::encode(&felt_bytes));
                decoded_strings.push(hex_repr);
            }
        }
    }

    decoded_strings
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

pub fn old_resource_bounds_mapping_to_resource_bounds_b_tree_map(
    resource_bounds_mapping: &starknet_old_types::ResourceBoundsMapping,
) -> ResourceBoundsMapping {
    ResourceBoundsMapping(BTreeMap::from([
        (
            Resource::L1Gas,
            ResourceBounds {
                max_amount: resource_bounds_mapping.l1_gas.max_amount,
                max_price_per_unit: resource_bounds_mapping.l1_gas.max_price_per_unit,
            },
        ),
        (
            Resource::L2Gas,
            ResourceBounds {
                max_amount: resource_bounds_mapping.l2_gas.max_amount,
                max_price_per_unit: resource_bounds_mapping.l2_gas.max_price_per_unit,
            },
        ),
    ]))
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

pub fn old_execution_result_to_execution_result(
    execution_result: starknet_old_types::ExecutionResult,
) -> ExecutionResult {
    match execution_result {
        starknet_old_types::ExecutionResult::Succeeded => ExecutionResult::Succeeded,
        starknet_old_types::ExecutionResult::Reverted { reason } => {
            ExecutionResult::Reverted { reason }
        }
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

pub fn old_deploy_contracts_to_deploy_contracts(
    old_deploy_contracts: Vec<starknet_old_types::DeployedContractItem>,
) -> Vec<DeployedContractItem> {
    old_deploy_contracts
        .into_iter()
        .map(|old_deploy_contract| DeployedContractItem {
            address: field_element_to_felt(old_deploy_contract.address),
            class_hash: field_element_to_felt(old_deploy_contract.class_hash),
        })
        .collect()
}

pub fn old_declared_classes_to_declared_classes(
    old_declared_classes: Vec<starknet_old_types::DeclaredClassItem>,
) -> Vec<DeclaredClassItem> {
    old_declared_classes
        .into_iter()
        .map(|old_declared_class| DeclaredClassItem {
            class_hash: field_element_to_felt(old_declared_class.class_hash),
            compiled_class_hash: field_element_to_felt(old_declared_class.compiled_class_hash),
        })
        .collect()
}

pub fn get_name_of_entry_point_selector(entry_point_selector: &Felt) -> Option<String> {
    let entry_point_selector_str = entry_point_selector.to_fixed_hex_string();
    let selector = get_selector(&entry_point_selector_str);
    match selector {
        Some(name) => Some(name.to_string()),
        None => None,
    }
}

pub fn parse_version_string_to_tuple(version: &str) -> Result<(u32, u32, u32)> {
    // Remove the leading 'v' if present
    let version_str = version.trim_start_matches('v');

    // Split the version string by '.'
    let parts: Vec<&str> = version_str.split('.').collect();

    // Ensure there are exactly three parts
    if parts.len() != 3 {
        return Err(anyhow::anyhow!("Invalid version string format"));
    }

    // Parse each part as a u32
    let major = parts[0]
        .parse::<u32>()
        .map_err(|_| anyhow::anyhow!("Invalid major version"))?;
    let minor = parts[1]
        .parse::<u32>()
        .map_err(|_| anyhow::anyhow!("Invalid minor version"))?;
    let patch = parts[2]
        .parse::<u32>()
        .map_err(|_| anyhow::anyhow!("Invalid patch version"))?;

    Ok((major, minor, patch))
}

pub fn tuple_to_version_string(version_tuple: (u32, u32, u32)) -> String {
    format!(
        "{}.{}.{}",
        version_tuple.0, version_tuple.1, version_tuple.2
    )
}

pub fn felt_str_to_fixed(felt_str: &str) -> anyhow::Result<String> {
    let felt = Felt::from_hex(felt_str)?;
    Ok(felt.to_fixed_hex_string())
}
