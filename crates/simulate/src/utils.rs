use crate::transaction_extraction::extract_chain_id_from_felt;
use crate::DebugPayload;
use crate::SimulationRawArgs;
use crate::TransactionSimulationError;
use starknet_api::executable_transaction::TransactionType;
use ethers::types::{Address, U256};
use num_bigint::{BigInt, BigUint};
use num_traits::Num;
use starknet::providers::{Provider, Url};
use starknet_api::block::BlockNumber;
use starknet_api::block::FeeType;
use starknet_api::core::{ChainId, ContractAddress, Nonce};
use starknet_api::transaction::fields::{Calldata, Fee};
use starknet_api::transaction::TransactionHash;
use starknet_api::transaction::TransactionVersion;
use starknet_types_core::felt::Felt;
use starknet_types_core::felt::CAIRO_PRIME_BIGINT;
use std::sync::Arc;
use walnut_shared::create_rpc_client_from_url;
use walnut_shared::extract_chain_id;
use walnut_shared::get_rpc_urls;

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
        "DEPLOY_ACCOUNT" => Ok(TransactionType::DeployAccount),
        "DEPLOY" => Err(TransactionSimulationError::TransactionTypeNotSupported), // DEPLOY is deprecated
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
