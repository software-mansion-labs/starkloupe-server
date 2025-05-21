use crate::contract_calls_map::ContractCallsMap;
use crate::transaction_extraction::extract_chain_id_from_felt;
use crate::DetailedTransactionReceipt;
use crate::FlameChartNode;
use crate::SimulationRawArgs;
use crate::TransactionSimulationError;
use blockifier::transaction::transaction_types::TransactionType;
use ethers::types::{Address, U256};
use num_bigint::{BigInt, BigUint};
use num_traits::Num;
use semver::Version;
use starknet::providers::{Provider, Url};
use starknet_api::block::BlockNumber;
use starknet_api::core::{ChainId, ContractAddress, Nonce};
use starknet_api::transaction::fields::Calldata;
use starknet_api::transaction::TransactionVersion;
use starknet_types_core::felt::Felt;
use starknet_types_core::felt::CAIRO_PRIME_BIGINT;
use std::sync::Arc;
use walnut_shared::create_rpc_client_from_url;
use walnut_shared::extract_chain_id;
use walnut_shared::get_rpc_urls;

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
        name: Some("Total resources".to_string()),
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
                name: Some("Computation Resources".to_string()),
                raw_value: detailed_tx_receipt
                    .computation_resources_gas_vector
                    .l2_gas
                    .0,
                value: 0.0,
                children: vec![
                    FlameChartNode {
                        call_id: 0,
                        name: Some("VM Cost".to_string()),
                        raw_value: detailed_tx_receipt
                            .computation_resources_vm_cost_gas_vector
                            .l2_gas
                            .0,
                        value: 0.0,
                        ..Default::default()
                    },
                    FlameChartNode {
                        call_id: 0,
                        name: Some("Sierra Cost".to_string()),
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
