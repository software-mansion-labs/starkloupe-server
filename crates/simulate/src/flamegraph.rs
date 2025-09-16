use crate::contract_calls_map::ContractCallsMap;
use crate::execution::PostExecStateData;
use crate::DetailedTransactionReceipt;
use crate::FlameChartNode;
use crate::FlameChartNodeType;
use blockifier::fee::eth_gas_constants::DATA_GAS_PER_FIELD_ELEMENT;
use blockifier::state::cached_state::StorageEntry;
use semver::Version;
use starknet_api::core::ContractAddress;
use std::collections::HashMap;
use std::collections::HashSet;
use walnut_shared::STRK_FEE_TOKEN_ADDRESS;

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

    pub fn from_state_changes(
        state_changes: &blockifier::state::cached_state::StateChanges,
    ) -> Self {
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
        node_type: Some(FlameChartNodeType::Root),
        children: vec![
            FlameChartNode {
                call_id: 0,
                name: Some("Starknet resources".to_string()),
                raw_value: detailed_tx_receipt.starknet_resources_gas_vector.l2_gas.0,
                value: 0.0,
                node_type: Some(FlameChartNodeType::Category),
                children: vec![
                    FlameChartNode {
                        call_id: 0,
                        name: Some("Archival resources".to_string()),
                        raw_value: detailed_tx_receipt
                            .starknet_resources_archival_data_gas_vector
                            .l2_gas
                            .0,
                        value: 0.0,
                        node_type: Some(FlameChartNodeType::Category),
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
                        node_type: Some(FlameChartNodeType::Category),
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
                        node_type: Some(FlameChartNodeType::Category),
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
                node_type: Some(FlameChartNodeType::Category),
                children: vec![
                    FlameChartNode {
                        call_id: 0,
                        name: Some("VM cost".to_string()),
                        raw_value: detailed_tx_receipt
                            .computation_resources_vm_cost_gas_vector
                            .l2_gas
                            .0,
                        value: 0.0,
                        node_type: Some(FlameChartNodeType::Category),
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
                        node_type: Some(FlameChartNodeType::Category),
                        children: contract_flamechart.to_vec(),
                    },
                ],
            },
        ],
    };

    normalize_with_sqrt(&mut root, contract_calls_map, 1.0);

    Some(root)
}

pub fn build_l1_data_flamegraph(
    post_exec_state_data: &PostExecStateData,
    detailed_tx_receipt: &DetailedTransactionReceipt,
) -> Option<FlameChartNode> {
    // Calculate on_chain_data_segment_length
    let n_modified_contracts = post_exec_state_data
        .starknet_resources_state
        .state_changes_for_fee
        .state_changes_count
        .n_modified_contracts as u64;

    let n_class_hash_updates = post_exec_state_data
        .starknet_resources_state
        .state_changes_for_fee
        .state_changes_count
        .n_class_hash_updates as u64;
    let n_storage_updates = post_exec_state_data
        .starknet_resources_state
        .state_changes_for_fee
        .state_changes_count
        .n_storage_updates as u64;
    let n_compiled_class_hash_updates = post_exec_state_data
        .starknet_resources_state
        .state_changes_for_fee
        .state_changes_count
        .n_compiled_class_hash_updates as u64;

    let on_chain_data_segment_length = n_modified_contracts * 2
        + n_class_hash_updates * 1
        + n_storage_updates * 2
        + n_compiled_class_hash_updates * 2;

    // Calculate da_gas_cost
    let da_gas_cost = on_chain_data_segment_length * DATA_GAS_PER_FIELD_ELEMENT as u64;

    // Calculate total_allocation_cost
    let n_allocated_keys = post_exec_state_data
        .starknet_resources_state
        .state_changes_for_fee
        .n_allocated_keys as u64;
    let total_allocation_cost = n_allocated_keys * DATA_GAS_PER_FIELD_ELEMENT as u64;

    // Storage updates: contract address -> storage key
    let mut contract_to_storage_keys: HashMap<String, Vec<String>> = HashMap::new();
    for ((contract_addr, storage_key), _) in &post_exec_state_data.state_changes.state_maps.storage
    {
        let contract_str = contract_addr.0.to_hex_string();
        let storage_key_str = storage_key.0.to_hex_string();
        contract_to_storage_keys
            .entry(contract_str)
            .or_default()
            .push(storage_key_str);
    }
    let storage_nodes: Vec<FlameChartNode> = contract_to_storage_keys
        .into_iter()
        .map(|(contract_addr, storage_keys)| FlameChartNode {
            call_id: 0,
            name: Some(contract_addr),
            raw_value: storage_keys.len() as u64 * 2 * DATA_GAS_PER_FIELD_ELEMENT as u64,
            value: 0.0,
            node_type: Some(FlameChartNodeType::ContractAddress),
            children: storage_keys
                .into_iter()
                .map(|storage_key| FlameChartNode {
                    call_id: 0,
                    name: Some(storage_key),
                    raw_value: 2 * DATA_GAS_PER_FIELD_ELEMENT as u64,
                    value: 0.0,
                    node_type: Some(FlameChartNodeType::StorageKey),
                    children: vec![],
                })
                .collect(),
        })
        .collect();

    // Class hash updates: contract address -> class_hash
    let mut contract_to_class_hash: HashMap<String, Vec<String>> = HashMap::new();
    for (contract_addr, class_hash) in &post_exec_state_data.state_changes.state_maps.class_hashes {
        let contract_str = contract_addr.0.to_hex_string();
        let class_hash_str = class_hash.0.to_hex_string();
        contract_to_class_hash
            .entry(contract_str)
            .or_default()
            .push(class_hash_str);
    }
    let class_hash_nodes: Vec<FlameChartNode> = contract_to_class_hash
        .into_iter()
        .map(|(contract_addr, class_hashes)| FlameChartNode {
            call_id: 0,
            name: Some(contract_addr),
            raw_value: class_hashes.len() as u64 * DATA_GAS_PER_FIELD_ELEMENT as u64,
            value: 0.0,
            node_type: Some(FlameChartNodeType::ContractAddress),
            children: class_hashes
                .into_iter()
                .map(|class_hash| FlameChartNode {
                    call_id: 0,
                    name: Some(class_hash),
                    raw_value: DATA_GAS_PER_FIELD_ELEMENT as u64,
                    value: 0.0,
                    node_type: Some(FlameChartNodeType::ClassHash),
                    children: vec![],
                })
                .collect(),
        })
        .collect();

    // Compiled class hash updates: contract address -> compiled_class_hash
    let mut contract_to_compiled_class_hash: HashMap<String, Vec<String>> = HashMap::new();
    for (class_hash, compiled_class_hash) in &post_exec_state_data
        .state_changes
        .state_maps
        .compiled_class_hashes
    {
        let class_hash_str = class_hash.0.to_hex_string();
        let compiled_class_hash_str = compiled_class_hash.0.to_hex_string();
        contract_to_compiled_class_hash
            .entry(class_hash_str)
            .or_default()
            .push(compiled_class_hash_str);
    }
    let compiled_class_hash_nodes: Vec<FlameChartNode> = contract_to_compiled_class_hash
        .into_iter()
        .map(|(class_hash, compiled_class_hashes)| FlameChartNode {
            call_id: 0,
            name: Some(class_hash),
            raw_value: compiled_class_hashes.len() as u64 * 2 * DATA_GAS_PER_FIELD_ELEMENT as u64,
            value: 0.0,
            node_type: Some(FlameChartNodeType::ClassHash),
            children: compiled_class_hashes
                .into_iter()
                .map(|compiled_class_hash| FlameChartNode {
                    call_id: 0,
                    name: Some(compiled_class_hash),
                    raw_value: 2 * DATA_GAS_PER_FIELD_ELEMENT as u64,
                    value: 0.0,
                    node_type: Some(FlameChartNodeType::ClassHash),
                    children: vec![],
                })
                .collect(),
        })
        .collect();

    // Allocation key cost: contract address -> storage key
    let allocated_keys_wrapper =
        AllocatedKeysWrapper::from_state_changes(&post_exec_state_data.state_changes);
    let mut contract_to_allocated_keys: HashMap<String, Vec<String>> = HashMap::new();
    for (contract_addr, storage_key) in &allocated_keys_wrapper.storage_keys {
        let contract_str = contract_addr.0.to_hex_string();
        let storage_key_str = storage_key.0.to_hex_string();
        contract_to_allocated_keys
            .entry(contract_str)
            .or_default()
            .push(storage_key_str);
    }
    let allocated_keys_nodes: Vec<FlameChartNode> = contract_to_allocated_keys
        .into_iter()
        .map(|(contract_addr, storage_keys)| FlameChartNode {
            call_id: 0,
            name: Some(contract_addr),
            raw_value: storage_keys.len() as u64 * DATA_GAS_PER_FIELD_ELEMENT as u64,
            value: 0.0,
            node_type: Some(FlameChartNodeType::ContractAddress),
            children: storage_keys
                .into_iter()
                .map(|storage_key| FlameChartNode {
                    call_id: 0,
                    name: Some(storage_key),
                    raw_value: DATA_GAS_PER_FIELD_ELEMENT as u64,
                    value: 0.0,
                    node_type: Some(FlameChartNodeType::StorageKey),
                    children: vec![],
                })
                .collect(),
        })
        .collect();

    let mut root = FlameChartNode {
        call_id: 0,
        raw_value: detailed_tx_receipt.gas.l1_data_gas.0,
        value: 1.0,
        name: Some("L1 Data Gas".to_string()),
        node_type: Some(FlameChartNodeType::Root),
        children: vec![
            // Data allocation cost
            FlameChartNode {
                call_id: 0,
                name: Some("Data allocation cost".to_string()),
                raw_value: da_gas_cost,
                value: 0.0,
                node_type: Some(FlameChartNodeType::Category),
                children: vec![
                    // Storage updates
                    FlameChartNode {
                        call_id: 0,
                        name: Some("Storage updates".to_string()),
                        raw_value: n_storage_updates * 2 * DATA_GAS_PER_FIELD_ELEMENT as u64,
                        value: 0.0,
                        node_type: Some(FlameChartNodeType::Category),
                        children: storage_nodes,
                    },
                    // Class hash updates
                    FlameChartNode {
                        call_id: 0,
                        name: Some("Class hash updates".to_string()),
                        raw_value: n_class_hash_updates * 1 * DATA_GAS_PER_FIELD_ELEMENT as u64,
                        value: 0.0,
                        node_type: Some(FlameChartNodeType::Category),
                        children: class_hash_nodes,
                    },
                    // Compiled class hash updates
                    FlameChartNode {
                        call_id: 0,
                        name: Some("Compiled class hash updates".to_string()),
                        raw_value: n_compiled_class_hash_updates
                            * 2
                            * DATA_GAS_PER_FIELD_ELEMENT as u64,
                        value: 0.0,
                        node_type: Some(FlameChartNodeType::Category),
                        children: compiled_class_hash_nodes,
                    },
                    // Modified contracts (ostaje kao ranije)
                    FlameChartNode {
                        call_id: 0,
                        name: Some("Modified contracts".to_string()),
                        raw_value: n_modified_contracts * 2 * DATA_GAS_PER_FIELD_ELEMENT as u64,
                        value: 0.0,
                        node_type: Some(FlameChartNodeType::Category),
                        children: {
                            let mut unique_contracts = std::collections::HashSet::new();
                            let starkgate_address = match starknet_types_core::felt::Felt::from_hex(
                                STRK_FEE_TOKEN_ADDRESS,
                            ) {
                                Ok(felt) => {
                                    match starknet_api::core::ContractAddress::try_from(felt) {
                                        Ok(addr) => addr,
                                        Err(_) => return None,
                                    }
                                }
                                Err(_) => return None,
                            };

                            // Add unique contract addresses from storage updates, excluding StarkGate
                            for (contract_addr, _) in
                                &post_exec_state_data.state_changes.state_maps.storage
                            {
                                if contract_addr.0 != starkgate_address {
                                    unique_contracts.insert(contract_addr.0.to_string());
                                }
                            }
                            unique_contracts
                                .into_iter()
                                .map(|contract_addr_str| FlameChartNode {
                                    call_id: 0,
                                    name: Some(contract_addr_str),
                                    raw_value: 2 * DATA_GAS_PER_FIELD_ELEMENT as u64,
                                    value: 0.0,
                                    node_type: Some(FlameChartNodeType::ContractAddress),
                                    children: vec![],
                                })
                                .collect()
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
                node_type: Some(FlameChartNodeType::Category),
                children: allocated_keys_nodes,
            },
        ],
    };

    normalize_with_sqrt(&mut root, &ContractCallsMap::new(), 1.0);

    Some(root)
}
