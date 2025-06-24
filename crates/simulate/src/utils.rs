use crate::contract_calls_map::ContractCallsMap;
use crate::execution::PostExecStateData;
use crate::transaction_extraction::extract_chain_id_from_felt;
use crate::DebugPayload;
use crate::DetailedTransactionReceipt;
use crate::FlameChartNode;
use crate::SimulationRawArgs;
use crate::TransactionSimulationError;
use blockifier::state::cached_state::StorageEntry;
use blockifier::fee::eth_gas_constants::DATA_GAS_PER_FIELD_ELEMENT;
use blockifier::transaction::transaction_types::TransactionType;
use ethers::types::{Address, U256};
use num_bigint::{BigInt, BigUint};
use num_traits::Num;
use semver::Version;
use starknet::providers::{Provider, Url};
use starknet_api::block::BlockNumber;
use starknet_api::block::FeeType;
use starknet_api::core::{ChainId, ContractAddress, Nonce};
use starknet_api::transaction::fields::{Calldata, Fee};
use starknet_api::transaction::TransactionHash;
use starknet_api::transaction::TransactionVersion;
use starknet_types_core::felt::Felt;
use starknet_types_core::felt::CAIRO_PRIME_BIGINT;
use std::collections::HashSet;
use std::sync::Arc;
use walnut_shared::create_rpc_client_from_url;
use walnut_shared::extract_chain_id;
use walnut_shared::get_rpc_urls;

// Wrapper struct to access AllocatedKeys
#[derive(Clone, Debug)]
pub struct AllocatedKeysWrapper {
    pub storage_keys: HashSet<(ContractAddress, starknet_api::state::StorageKey)>,
}

impl AllocatedKeysWrapper {
    pub fn new(allocated_keys: &blockifier::state::cached_state::AllocatedKeys) -> Self {
        // Use unsafe transmute to access the private field
        // This is "safe" because we know the internal structure of AllocatedKeys
        unsafe {
            let storage_entries: &std::collections::HashSet<StorageEntry> = 
                std::mem::transmute(allocated_keys);
            let mut storage_keys = HashSet::new();
            for storage_entry in storage_entries {
                storage_keys.insert((storage_entry.0, storage_entry.1));
            }
            Self { storage_keys }
        }
    }
    
    pub fn from_state_changes(state_changes: &blockifier::state::cached_state::StateChanges) -> Self {
        // Use unsafe transmute to access the private field of allocated_keys
        unsafe {
            let storage_entries: &std::collections::HashSet<StorageEntry> = 
                std::mem::transmute(&state_changes.allocated_keys);
            let mut storage_keys = HashSet::new();
            for storage_entry in storage_entries {
                storage_keys.insert((storage_entry.0, storage_entry.1));
            }
            Self { storage_keys }
        }
    }
}

pub async fn parse_chain_id_and_rpc_url_debug(
    raw_args: &DebugPayload,
) -> Result<(ChainId, Url), TransactionSimulationError> {
    if let Some(chain_id_str) = &raw_args.chain_id {
        let (e_chain_id, _network) = extract_chain_id(chain_id_str)
            .map_err(|_| TransactionSimulationError::InvalidChainId)?;
        let rpc_url = get_rpc_urls(&e_chain_id)
            .0
            .ok_or(TransactionSimulationError::InvalidRpcUrl)?;
        let core_chain_id = ChainId::from(e_chain_id);
        Ok((core_chain_id, rpc_url))
    } else if let Some(rpc_url_str) = &raw_args.rpc_url {
        let rpc_url =
            Url::parse(rpc_url_str).map_err(|_| TransactionSimulationError::InvalidRpcUrl)?;
        let provider_client = create_rpc_client_from_url(rpc_url.clone());
        let chain_id_felt = provider_client
            .chain_id()
            .await
            .map_err(|_| TransactionSimulationError::FailedToFetchChainId)?;
        let core_chain_id = extract_chain_id_from_felt(chain_id_felt)?;
        Ok((core_chain_id, rpc_url))
    } else {
        Err(TransactionSimulationError::MissingChainIdOrRpcUrl)
    }
}

pub async fn parse_chain_id_and_rpc_url(
    raw_args: &SimulationRawArgs,
) -> Result<(ChainId, Url), TransactionSimulationError> {
    if let Some(chain_id_str) = &raw_args.chain_id {
        let (e_chain_id, _network) = extract_chain_id(chain_id_str)
            .map_err(|_| TransactionSimulationError::InvalidChainId)?;
        let rpc_url = get_rpc_urls(&e_chain_id)
            .0
            .ok_or(TransactionSimulationError::InvalidRpcUrl)?;
        let core_chain_id = ChainId::from(e_chain_id);
        Ok((core_chain_id, rpc_url))
    } else if let Some(rpc_url_str) = &raw_args.rpc_url {
        let rpc_url =
            Url::parse(rpc_url_str).map_err(|_| TransactionSimulationError::InvalidRpcUrl)?;
        let provider_client = create_rpc_client_from_url(rpc_url.clone());
        let chain_id_felt = provider_client
            .chain_id()
            .await
            .map_err(|_| TransactionSimulationError::FailedToFetchChainId)?;
        let core_chain_id = extract_chain_id_from_felt(chain_id_felt)?;
        Ok((core_chain_id, rpc_url))
    } else {
        Err(TransactionSimulationError::MissingChainIdOrRpcUrl)
    }
}

pub fn parse_optional_tx_hash(
    hash_str_opt: Option<&str>,
) -> Result<Option<TransactionHash>, TransactionSimulationError> {
    match hash_str_opt {
        Some(s) => {
            let felt =
                parse_felt(s).map_err(|_| TransactionSimulationError::InvalidTransactionHash)?;
            Ok(Some(TransactionHash(felt)))
        }
        None => Ok(None),
    }
}

fn parse_felt(value: &str) -> Result<Felt, ()> {
    Felt::from_hex(value).map_err(|_| ())
}

pub fn parse_transaction_type(value: &str) -> Result<TransactionType, TransactionSimulationError> {
    match value.to_uppercase().as_str() {
        "INVOKE" => Ok(TransactionType::InvokeFunction),
        "DECLARE" => Ok(TransactionType::Declare),
        "L1HANDLER" => Ok(TransactionType::L1Handler),
        _ => Err(TransactionSimulationError::TransactionTypeNotSupported),
    }
}

pub fn parse_nonce(nonce_opt: Option<u64>) -> Option<Nonce> {
    nonce_opt.map(|nonce| Nonce(Felt::from(nonce)))
}

pub fn parse_block_number(block_number_opt: Option<u64>) -> Option<BlockNumber> {
    block_number_opt.map(BlockNumber)
}

pub fn parse_contract_address(
    sender_address: &str,
) -> Result<ContractAddress, TransactionSimulationError> {
    let hex_str = sender_address.trim_start_matches("0x");
    let bigint_value = BigInt::from_str_radix(hex_str, 16)
        .map_err(|_| TransactionSimulationError::InvalidContractAddress)?;
    if bigint_value >= *CAIRO_PRIME_BIGINT {
        return Err(TransactionSimulationError::InvalidContractAddress);
    }
    let felt_value = Felt::from(bigint_value);
    ContractAddress::try_from(felt_value)
        .map_err(|_| TransactionSimulationError::InvalidContractAddress)
}

pub fn parse_calldata(calldata_strings: &[String]) -> Result<Calldata, TransactionSimulationError> {
    let calldata_vec = calldata_strings
        .iter()
        .map(|x| {
            let hex_str = x.trim_start_matches("0x");
            let bigint_value = BigInt::from_str_radix(hex_str, 16)
                .map_err(|_| TransactionSimulationError::InvalidCalldata)?;
            if bigint_value >= *CAIRO_PRIME_BIGINT {
                Err(TransactionSimulationError::InvalidCalldata)
            } else {
                Ok(Felt::from(bigint_value))
            }
        })
        .collect::<Result<Vec<Felt>, TransactionSimulationError>>()?;
    Ok(Calldata(Arc::new(calldata_vec)))
}

pub fn parse_transaction_version(
    version: usize,
) -> Result<TransactionVersion, TransactionSimulationError> {
    match version {
        0 => Ok(TransactionVersion::ZERO),
        1 => Ok(TransactionVersion::ONE),
        2 => Ok(TransactionVersion::TWO),
        3 => Ok(TransactionVersion::THREE),
        _ => Err(TransactionSimulationError::InvalidTransactionVersion),
    }
}

pub fn convert_to_hex(num_str: &str) -> String {
    let num = BigUint::from_str_radix(num_str, 10).unwrap();
    format!("{:x}", num)
}

pub fn transaction_type_to_string(tx_type: TransactionType) -> String {
    match tx_type {
        TransactionType::Declare => "DECLARE".to_string(),
        TransactionType::DeployAccount => "DEPLOY".to_string(),
        TransactionType::InvokeFunction => "INVOKE".to_string(),
        TransactionType::L1Handler => "L1HANDLER".to_string(),
    }
}

pub fn calldata_to_hex(calldata: &Calldata) -> Vec<String> {
    calldata
        .0
        .iter()
        .map(|felt| felt.to_hex_string())
        .collect::<Vec<String>>()
}

pub fn eth_address_to_felt(addr: Address) -> Felt {
    let eth_address_as_bytes = addr.as_bytes();
    let mut bytes: [u8; 32] = [0; 32];
    bytes[12..32].copy_from_slice(eth_address_as_bytes);
    Felt::from_bytes_be(&bytes)
}

pub fn eth_u256_to_felt(value: U256) -> Felt {
    let mut bytes = [0u8; 32];
    value.to_big_endian(&mut bytes);
    Felt::from_bytes_be(&bytes)
}

fn format_flamechart_node_name(
    contract_name: &Option<String>,
    erc20_token_name: &Option<String>,
    erc20_token_symbol: &Option<String>,
    entry_point_interface_name: &Option<String>,
    entry_point_name: &Option<String>,
    entry_point_selector: &Option<String>,
    contract_address: &str,
) -> String {
    let suffix = entry_point_name
        .as_deref()
        .or(entry_point_selector.as_deref())
        .unwrap_or_default();

    if let Some(name) = contract_name.as_deref() {
        return format!("{}.{}", name, suffix);
    }

    if erc20_token_name.is_some() || erc20_token_symbol.is_some() {
        let name = erc20_token_name.as_deref().unwrap_or_default();
        let symbol = erc20_token_symbol
            .as_deref()
            .map(|s| format!(" ({})", s))
            .unwrap_or_default();
        return format!("{}{}.{}", name, symbol, suffix);
    }

    if let Some(interface) = entry_point_interface_name.as_deref() {
        if let Some(last_part) = interface.rsplit("::").next() {
            return format!("{}.{}", last_part, suffix);
        }
    }

    format!("{}.{}", contract_address, suffix)
}

fn normalize_with_sqrt(
    node: &mut FlameChartNode,
    contract_calls_map: &ContractCallsMap,
    parent_value: f64,
) {
    if let Some(contract_call) = contract_calls_map.0.get(&node.call_id) {
        node.name = Some(format_flamechart_node_name(
            &contract_call.contract_name,
            &contract_call.erc20_token_name,
            &contract_call.erc20_token_symbol,
            &contract_call.entry_point_interface_name,
            &contract_call.entry_point_name,
            &contract_call.entry_point_selector,
            &contract_call.entry_point.storage_address.0.to_hex_string(),
        ));
    }

    if !node.children.is_empty() {
        let sqrt_sum: f64 = node
            .children
            .iter()
            .map(|c| (c.raw_value as f64).sqrt())
            .sum();

        for child in &mut node.children {
            let sqrt_val = (child.raw_value as f64).sqrt();
            child.value = if sqrt_sum > 1e-8 {
                parent_value * (sqrt_val / sqrt_sum)
            } else {
                0.0
            };

            normalize_with_sqrt(child, contract_calls_map, child.value);
        }
    }
}

pub fn build_flamegraph(
    detailed_tx_receipt: &DetailedTransactionReceipt,
    contract_calls_map: &ContractCallsMap,
    contract_flamechart: &mut [FlameChartNode],
) -> Option<FlameChartNode> {
    // Early return if any call has sierra_version < 1.7.0
    if contract_flamechart
        .iter()
        .any(|node| contains_old_sierra_version(node, contract_calls_map))
    {
        return None;
    }

    let mut root = FlameChartNode {
        call_id: 0,
        raw_value: detailed_tx_receipt.gas.l2_gas.0,
        value: 1.0,
        name: Some("L2 Gas".to_string()),
        children: vec![
            FlameChartNode {
                call_id: 0,
                name: Some("Starknet resources".to_string()),
                raw_value: detailed_tx_receipt.starknet_resources_gas_vector.l2_gas.0,
                value: 0.0,
                children: vec![
                    FlameChartNode {
                        call_id: 0,
                        name: Some("Archival resources".to_string()),
                        raw_value: detailed_tx_receipt
                            .starknet_resources_archival_data_gas_vector
                            .l2_gas
                            .0,
                        value: 0.0,
                        ..Default::default()
                    },
                    FlameChartNode {
                        call_id: 0,
                        name: Some("Messages resources".to_string()),
                        raw_value: detailed_tx_receipt
                            .starknet_resources_message_gas_vector
                            .l2_gas
                            .0,
                        value: 0.0,
                        ..Default::default()
                    },
                    FlameChartNode {
                        call_id: 0,
                        name: Some("State resources".to_string()),
                        raw_value: detailed_tx_receipt
                            .starknet_resources_state_gas_vector
                            .l2_gas
                            .0,
                        value: 0.0,
                        ..Default::default()
                    },
                ],
            },
            FlameChartNode {
                call_id: 0,
                name: Some("Computation resources".to_string()),
                raw_value: detailed_tx_receipt
                    .computation_resources_gas_vector
                    .l2_gas
                    .0,
                value: 0.0,
                children: vec![
                    FlameChartNode {
                        call_id: 0,
                        name: Some("VM cost".to_string()),
                        raw_value: detailed_tx_receipt
                            .computation_resources_vm_cost_gas_vector
                            .l2_gas
                            .0,
                        value: 0.0,
                        ..Default::default()
                    },
                    FlameChartNode {
                        call_id: 0,
                        name: Some("Sierra cost".to_string()),
                        raw_value: detailed_tx_receipt
                            .computation_resources_sierra_gas_vector
                            .l2_gas
                            .0,
                        value: 0.0,
                        children: contract_flamechart.to_vec(),
                    },
                ],
            },
        ],
    };

    normalize_with_sqrt(&mut root, contract_calls_map, 1.0);

    Some(root)
}

fn contains_old_sierra_version(
    node: &FlameChartNode,
    contract_calls_map: &ContractCallsMap,
) -> bool {
    if let Some(contract_call) = contract_calls_map.0.get(&node.call_id) {
        if let Some(version) = &contract_call.sierra_version {
            if is_version_less_than(version, "1.7.0") {
                return true;
            }
        }
    }

    node.children
        .iter()
        .any(|child| contains_old_sierra_version(child, contract_calls_map))
}

fn is_version_less_than(version: &str, threshold: &str) -> bool {
    Version::parse(version)
        .and_then(|v| Version::parse(threshold).map(|t| v < t))
        .unwrap_or(false)
}

pub fn format_fee_string(fee: Fee, fee_type: FeeType) -> Option<String> {
    if fee.0 == 0 {
        return None;
    }

    let decimals = 10u128.pow(18);
    let whole = fee.0 / decimals;
    let fraction = fee.0 % decimals;

    let formatted = format!(
        "{}.{:018} {}",
        whole,
        fraction,
        match fee_type {
            FeeType::Strk => "STRK",
            FeeType::Eth => "ETH",
        }
    );

    Some(formatted)
}

pub fn build_l1_data_flamegraph(
    post_exec_state_data: &PostExecStateData,
    detailed_tx_receipt: &DetailedTransactionReceipt,
) -> Option<FlameChartNode> {
    
    // Calculate on_chain_data_segment_length
    let n_modified_contracts = post_exec_state_data.starknet_resources_state.state_changes_for_fee.state_changes_count.n_modified_contracts as u64;
    
    let n_class_hash_updates = post_exec_state_data.starknet_resources_state.state_changes_for_fee.state_changes_count.n_class_hash_updates as u64;
    let n_storage_updates = post_exec_state_data.starknet_resources_state.state_changes_for_fee.state_changes_count.n_storage_updates as u64;
    let n_compiled_class_hash_updates = post_exec_state_data.starknet_resources_state.state_changes_for_fee.state_changes_count.n_compiled_class_hash_updates as u64;
    
    let on_chain_data_segment_length = n_modified_contracts * 2 + n_class_hash_updates * 1 + n_storage_updates * 2 + n_compiled_class_hash_updates * 2;

    // Calculate da_gas_cost
    let da_gas_cost = on_chain_data_segment_length * DATA_GAS_PER_FIELD_ELEMENT as u64;

    // Calculate total_allocation_cost
    let n_allocated_keys = post_exec_state_data.starknet_resources_state.state_changes_for_fee.n_allocated_keys as u64;
    let total_allocation_cost = n_allocated_keys * DATA_GAS_PER_FIELD_ELEMENT as u64;
    
    let mut root = FlameChartNode {
        call_id: 0,
        raw_value: detailed_tx_receipt.gas.l1_data_gas.0,
        value: 1.0,
        name: Some("L1 Data Gas".to_string()),
        children: vec![
            // Data allocation cost
            FlameChartNode {
                call_id: 0,
                name: Some("Data allocation cost".to_string()),
                raw_value: da_gas_cost,
                value: 0.0,
                children: vec![
                    // Storage updates
                    FlameChartNode {
                        call_id: 0,
                        name: Some("Storage updates".to_string()),
                        raw_value: n_storage_updates * 2 * DATA_GAS_PER_FIELD_ELEMENT as u64,
                        value: 0.0,
                        children: post_exec_state_data.state_changes.state_maps.storage.keys().map(|(_contract_addr, storage_key)| {
                            FlameChartNode {
                                call_id: 0,
                                name: Some(storage_key.0.to_string()),
                                raw_value: 2 * DATA_GAS_PER_FIELD_ELEMENT as u64, 
                                value: 0.0,
                                children: vec![],
                            }
                        }).collect(),
                    },
                    // Class hash updates
                    FlameChartNode {
                        call_id: 0,
                        name: Some("Class hash updates".to_string()),
                        raw_value: n_class_hash_updates * 1 * DATA_GAS_PER_FIELD_ELEMENT as u64,
                        value: 0.0,
                        children: post_exec_state_data.state_changes.state_maps.class_hashes.iter().map(|(_contract_addr, class_hash)| {
                            FlameChartNode {
                                call_id: 0,
                                name: Some(class_hash.0.to_hex_string()),
                                raw_value: 1 * DATA_GAS_PER_FIELD_ELEMENT as u64, 
                                value: 0.0,
                                children: vec![],
                            }
                        }).collect(),
                    },
                    // Compiled class hash updates
                    FlameChartNode {
                        call_id: 0,
                        name: Some("Compiled class hash updates".to_string()),
                        raw_value: n_compiled_class_hash_updates * 2 * DATA_GAS_PER_FIELD_ELEMENT as u64,
                        value: 0.0,
                        children: post_exec_state_data.state_changes.state_maps.compiled_class_hashes.iter().map(|(_class_hash, compiled_class_hash)| {
                            FlameChartNode {
                                call_id: 0,
                                name: Some(compiled_class_hash.0.to_hex_string()),
                                raw_value: 2 * DATA_GAS_PER_FIELD_ELEMENT as u64,
                                value: 0.0,
                                children: vec![],
                            }
                        }).collect(),
                    },
                    // Modified contracts
                    FlameChartNode {
                        call_id: 0,
                        name: Some("Modified contracts".to_string()),
                        raw_value: n_modified_contracts * 2 * DATA_GAS_PER_FIELD_ELEMENT as u64,
                        value: 0.0,
                        children: {
                            let mut unique_contracts = std::collections::HashSet::new();
                            let starkgate_address = starknet_api::core::ContractAddress::try_from(
                                starknet_types_core::felt::Felt::from_hex("4718f5a0fc34cc1af16a1cdee98ffb20c31f5cd61d6ab07201858f4287c938d").unwrap()
                            ).unwrap();
                            
                            // Add unique contract addresses from storage updates, excluding StarkGate
                            for (contract_addr, _) in &post_exec_state_data.state_changes.state_maps.storage {
                                if contract_addr.0 != starkgate_address {
                                    unique_contracts.insert(contract_addr.0.to_string());
                                }
                            }
                            unique_contracts.into_iter().map(|contract_addr_str| {
                                FlameChartNode {
                                    call_id: 0,
                                    name: Some(contract_addr_str),
                                    raw_value: 2 * DATA_GAS_PER_FIELD_ELEMENT as u64,
                                    value: 0.0,
                                    children: vec![],
                                }
                            }).collect()
                        },
                    },
                ],
            },
            // Allocation key cost
            FlameChartNode {
                call_id: 0,
                name: Some("Allocation key cost".to_string()),
                raw_value: total_allocation_cost,
                value: 0.0,
                children: {
                    let allocated_keys_wrapper = AllocatedKeysWrapper::from_state_changes(&post_exec_state_data.state_changes);
                    allocated_keys_wrapper.storage_keys.iter().map(|(_contract_addr, storage_key)| {
                        FlameChartNode {
                            call_id: 0,
                            name: Some(storage_key.0.to_string()),
                            raw_value: DATA_GAS_PER_FIELD_ELEMENT as u64, 
                            value: 0.0,
                            children: vec![],
                        }
                    }).collect()
                },
            },
        ],
    };

    normalize_with_sqrt(&mut root, &ContractCallsMap::new(), 1.0);

    Some(root)
}
