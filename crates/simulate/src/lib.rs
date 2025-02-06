pub mod abi_processor;
pub mod contract_call;
pub mod contract_calls_map;
pub mod contract_names;
pub mod debugger_trace;
pub mod events;
pub mod function_calls;
pub mod simulate;
pub mod state;
pub mod transaction_extraction;
pub mod transaction_info;
pub mod utils;
use blockifier::execution::errors::EntryPointExecutionError;
use blockifier::state::errors::StateError;
use blockifier::transaction::errors::TransactionExecutionError;
use blockifier::transaction::transaction_types::TransactionType;
use contract_call::ContractCall;
use contract_calls_map::ContractCallsMap;
use events::EmittedEvent;
use internal_tracing::function_calls_map::FunctionCallsMap;
use internal_tracing::SimulationDebuggerData;
use serde::Deserialize;
use serde::Serialize;
use serde::Serializer;
use starknet::core::types::ExecutionResult;
use starknet::core::types::Felt;
use starknet_api::block::BlockNumber;
use starknet_api::core::{ChainId, ContractAddress, Nonce};
use starknet_api::transaction::Calldata;
use starknet_api::transaction::PaymasterData;
use starknet_api::transaction::ResourceBoundsMapping;
use starknet_api::transaction::TransactionHash;
use starknet_api::transaction::TransactionSignature;
use starknet_api::transaction::TransactionVersion;
use starknet_old::core::types as starknet_old_types;
use starknet_providers::ProviderError;
use starknet_types_core::felt::FromStrError;
use thiserror::Error;
use tracing::error;
use url::Url;
use utils::{
    parse_block_number, parse_calldata, parse_chain_id_and_rpc_url, parse_contract_address,
    parse_nonce, parse_transaction_version,
};
use walnut_shared::EventAbi;
use walnut_shared::Parameter;

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
    pub chain_id: ChainId,
    pub rpc_url: Url,
    pub block_number: Option<BlockNumber>,
    pub nonce: Option<Nonce>,
    pub sender_address: ContractAddress,
    pub calldata: Calldata,
    pub transaction_version: TransactionVersion,
    pub transaction_signature: Option<TransactionSignature>,
    pub transaction_hash: Option<TransactionHash>,
    pub transaction_type: Option<TransactionType>,
    pub resource_bounds: Option<ResourceBoundsMapping>,
    pub paymaster_data: Option<PaymasterData>,
}

impl SimulationArgs {
    pub async fn try_from_raw_args(
        raw_args: SimulationRawArgs,
    ) -> Result<Self, TransactionSimulationError> {
        let (chain_id, rpc_url) = parse_chain_id_and_rpc_url(&raw_args).await?;
        let nonce = parse_nonce(raw_args.nonce);
        let block_number = parse_block_number(raw_args.block_number);
        let sender_address = parse_contract_address(&raw_args.sender_address)?;
        let calldata = parse_calldata(&raw_args.calldata)?;
        let transaction_version = parse_transaction_version(raw_args.transaction_version)?;

        Ok(Self {
            chain_id,
            rpc_url,
            block_number,
            nonce,
            sender_address,
            calldata,
            transaction_version,
            transaction_signature: None,
            transaction_hash: None,
            transaction_type: None,
            resource_bounds: None,
            paymaster_data: None,
        })
    }
}

#[derive(Serialize, Debug)]
pub struct TransactionSimulationResult {
    pub simulation_result: SimulationInfo,
    pub chain_id: String,
    #[serde(serialize_with = "serialize_block_number")]
    pub block_number: starknet_old_types::BlockId,
    pub block_timestamp: u64,
    pub nonce: Option<u64>,
    pub sender_address: String,
    pub calldata: Vec<String>,
    pub transaction_version: usize,
    pub transaction_type: String,
    pub transaction_index_in_block: Option<usize>,
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
    #[error("Invalid contract address")]
    InvalidContractAddress,
    #[error("Invalid Calldata")]
    InvalidCalldata,
    #[error("Invalid transaction hash")]
    InvalidTransactionHash,
    #[error("Transaction type is not supported")]
    TransactionTypeNotSupported,
    #[error("Either chain_id or rpc_url must be provided")]
    MissingChainIdOrRpcUrl,
    #[error("Transaction index can not be extracted from block")]
    TransactionIndexNotFound,
    #[error("Invalid transaction version")]
    InvalidTransactionVersion,
    #[error("Failed to fetch chain id")]
    FailedToFetchChainId,
    #[error("Failed to decode chain id")]
    FailedToDecodeChainId,
    #[error("Error occurred: {0}")]
    OtherError(String),
}

fn serialize_block_number<S>(
    block_id: &starknet_old_types::BlockId,
    serializer: S,
) -> Result<S::Ok, S::Error>
where
    S: Serializer,
{
    match block_id {
        starknet_old_types::BlockId::Number(num) => serializer.serialize_u64(*num),
        starknet_old_types::BlockId::Hash(hash) => serializer.serialize_str(&format!("{:?}", hash)),
        starknet_old_types::BlockId::Tag(tag) => serializer.serialize_str(&format!("{:?}", tag)),
    }
}

#[derive(Serialize, Debug)]
pub struct SimulationInfo {
    pub contract_calls_map: ContractCallsMap,
    pub function_calls_map: FunctionCallsMap,
    pub events: Vec<EmittedEvent>,
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
