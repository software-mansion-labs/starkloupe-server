use crate::transaction_extraction::extract_chain_id_from_felt;
use crate::SimulationRawArgs;
use crate::TransactionSimulationError;
use blockifier::transaction::transaction_types::TransactionType;
use num_bigint::{BigInt, BigUint};
use num_traits::Num;
use starknet_api::block::BlockNumber;
use starknet_api::core::{ChainId, ContractAddress, Nonce};
use starknet_api::transaction::Calldata;
use starknet_api::transaction::TransactionVersion;
use starknet_providers::Provider;
use starknet_types_core::felt::Felt;
use starknet_types_core::felt::CAIRO_PRIME_BIGINT;
use std::sync::Arc;
use url::Url;
use walnut_shared::create_rpc_client_from_url;
use walnut_shared::field_element_to_felt;
use walnut_shared::{extract_chain_id, rpc_url};

pub async fn parse_chain_id_and_rpc_url(
    raw_args: &SimulationRawArgs,
) -> Result<(ChainId, Url), TransactionSimulationError> {
    if let Some(chain_id_str) = &raw_args.chain_id {
        let chain_id = extract_chain_id(chain_id_str)
            .map_err(|_| TransactionSimulationError::InvalidChainId)?;
        let rpc_url = rpc_url(&chain_id);
        Ok((chain_id, rpc_url))
    } else if let Some(rpc_url_str) = &raw_args.rpc_url {
        let rpc_url =
            Url::parse(rpc_url_str).map_err(|_| TransactionSimulationError::InvalidRpcUrl)?;
        let provider_client = create_rpc_client_from_url(rpc_url.clone());
        let chain_id_felt = field_element_to_felt(
            provider_client
                .chain_id()
                .await
                .map_err(|_| TransactionSimulationError::FailedToFetchChainId)?,
        );
        let chain_id = extract_chain_id_from_felt(chain_id_felt)?;
        Ok((chain_id, rpc_url))
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

pub fn parse_transaction_hash(tx_hash: &str) -> Result<Felt, TransactionSimulationError> {
    let hex_str = tx_hash.trim_start_matches("0x");
    let bigint_value = BigInt::from_str_radix(hex_str, 16)
        .map_err(|_| TransactionSimulationError::InvalidTransactionHash)?;
    if bigint_value >= *CAIRO_PRIME_BIGINT {
        return Err(TransactionSimulationError::InvalidTransactionHash);
    }
    Ok(Felt::from(bigint_value))
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
        TransactionType::L1Handler => "L1Handler".to_string(),
    }
}
