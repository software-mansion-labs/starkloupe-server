pub mod abi_processor;
pub mod contract_call;
pub mod contract_calls_map;
pub mod contract_names;
pub mod debugger_trace;
pub mod event_abi;
pub mod function_calls;
pub mod simulate;
pub mod state;
pub mod transaction_extraction;
pub mod utils;

use blockifier::execution::errors::EntryPointExecutionError;
use blockifier::state::errors::StateError;
use blockifier::transaction::errors::TransactionExecutionError;
use contract_call::ContractCall;
use contract_calls_map::ContractCallsMap;
use internal_tracing::function_calls_map::FunctionCallsMap;
use internal_tracing::SimulationDebuggerData;
use serde::Deserialize;
use serde::Serialize;
use starknet::core::types::ExecutionResult;
use starknet::core::types::Felt;
use starknet_api::block::BlockNumber;
use starknet_api::core::{ChainId, ContractAddress, Nonce, PatriciaKey};
use starknet_api::transaction::TransactionHash;
use starknet_api::transaction::TransactionSignature;
use starknet_api::transaction::TransactionVersion;
use starknet_api::{contract_address, felt, patricia_key};
use starknet_providers::ProviderError;
use starknet_types_core::felt::FromStrError;
use thiserror::Error;
use tracing::error;
use url::Url;
use walnut_shared::EventAbi;
use walnut_shared::Parameter;
use walnut_shared::{extract_chain_id, rpc_url};

#[derive(Serialize, Deserialize, Debug)]
pub struct SimulationRawArgs {
    pub chain_id: Option<String>,
    pub rpc_url: Option<String>,
    pub block_number: Option<u64>,
    pub nonce: Option<u64>,
    pub sender_address: String,
    pub calldata: Vec<String>,
    pub transaction_version: usize,
    pub transaction_signature: Option<Vec<Felt>>,
}

#[derive(Debug)]
pub struct SimulationArgs {
    pub chain_id: Option<ChainId>,
    pub rpc_url: Url,
    pub block_number: BlockNumber,
    pub nonce: Option<Nonce>,
    pub sender_address: ContractAddress,
    pub calldata: Vec<Felt>,
    pub transaction_version: TransactionVersion,
    pub transaction_signature: Option<TransactionSignature>,
    pub transaction_hash: Option<TransactionHash>,
}

#[derive(Serialize, Debug)]
pub struct TransactionSimulationResult {
    pub simulation_result: SimulationInfo,
    pub chain_id: Option<String>,
    pub block_number: u64,
    pub block_timestamp: u64,
    pub nonce: Option<u64>,
    pub sender_address: String,
    pub calldata: Vec<String>,
    pub transaction_version: usize,
    pub transaction_type: String,
    pub transaction_index_in_block: usize,
}

#[derive(Error, Debug)]
pub enum TransactionSimulationError {
    #[error("{0}")]
    EntryPointExecutionError(#[from] EntryPointExecutionError),
    #[error("{0}")]
    StateError(#[from] StateError),
    #[error("{0}")]
    ProviderError(#[from] ProviderError),
    #[error("{0}")]
    PendingBlock(String),
    #[error("{0}")]
    TransactionExecutionError(#[from] TransactionExecutionError),
    #[error("Invalid Felt string conversion: {0}")]
    FeltConversionError(#[from] FromStrError),
    #[error("Transaction hash not found")]
    TransactionHashNotFound,
    #[error("Invalid chain id")]
    InvalidChainId,
    #[error("Invalid RPC URL")]
    InvalidRpcUrl,
    #[error("Either chain_id or rpc_url must be provided")]
    MissingChainIdOrRpcUrl,
    #[error("Transaction index can not be extracted from block")]
    TransactionIndexNotFound,
    #[error("Invalid transaction version")]
    InvalidTransactionVersion,
    #[error("Error occurred: {0}")]
    OtherError(String),
}

impl TryFrom<SimulationRawArgs> for SimulationArgs {
    type Error = TransactionSimulationError;

    fn try_from(raw_args: SimulationRawArgs) -> Result<Self, Self::Error> {
        let mut chain_id: Option<ChainId> = None;

        let rpc_url = if let Some(chain_id_str) = raw_args.chain_id {
            let extracted_chain_id = extract_chain_id(chain_id_str.as_str())
                .map_err(|_| TransactionSimulationError::InvalidChainId)?;
            chain_id = Some(extracted_chain_id.clone());
            rpc_url(&extracted_chain_id)
        } else if let Some(rpc_url) = raw_args.rpc_url {
            Url::parse(&rpc_url).map_err(|_| TransactionSimulationError::InvalidRpcUrl)?
        } else {
            return Err(TransactionSimulationError::MissingChainIdOrRpcUrl);
        };

        let calldata: Vec<Felt> = raw_args
            .calldata
            .iter()
            .map(|x| felt!(x.as_str()))
            .collect();

        Ok(Self {
            chain_id,
            rpc_url,
            block_number: raw_args
                .block_number
                .map_or(BlockNumber::default(), BlockNumber),
            nonce: raw_args.nonce.map(|nonce| Nonce(Felt::from(nonce))),
            sender_address: contract_address!(raw_args.sender_address.as_str()),
            calldata,
            transaction_version: match raw_args.transaction_version {
                0 => TransactionVersion::ZERO,
                1 => TransactionVersion::ONE,
                2 => TransactionVersion::TWO,
                3 => TransactionVersion::THREE,
                _ => {
                    return Err(TransactionSimulationError::InvalidTransactionVersion);
                }
            },
            transaction_signature: None,
            transaction_hash: None,
        })
    }
}

#[derive(Serialize, Debug)]
pub struct SimulationInfo {
    pub contract_calls_map: ContractCallsMap,
    pub function_calls_map: FunctionCallsMap,
    pub events: Vec<ContractCallEvent>,
    pub execution_result: ExecutionResult,
    pub simulation_debugger_data: Option<SimulationDebuggerData>,
}
#[derive(Serialize, Debug)]
pub struct ContractCallEvent {
    pub contract_call_id: u32,
    pub name: String,
    pub keys: Vec<String>,
    pub parameters: Vec<Parameter>,
    pub data: Vec<String>,
}
