pub mod contract_call;
pub mod contract_calls_map;
pub mod contract_names;
pub mod debugger;
pub mod debugger_trace;
pub mod events;
pub mod execution;
pub mod function_calls;
pub mod gas_counter;
pub mod simulate;
pub mod state;
pub mod storage_changes;
pub mod transaction_extraction;
pub mod transaction_info;
pub mod utils;
use blockifier::execution::errors::EntryPointExecutionError;
use blockifier::fee::fee_checks::FeeCheckError;
use blockifier::state::errors::StateError;
use blockifier::transaction::errors::TransactionExecutionError;
use blockifier::transaction::transaction_types::TransactionType;
use contract_call::ContractCall;
use contract_calls_map::ContractCallsMap;
use ethers::types::{Address, U256};
use events::EmittedEvent;
use internal_tracing::event_calls_map::EventCallsMap;
use internal_tracing::function_calls_map::FunctionCallsMap;
use internal_tracing::SimulationDebuggerData;
use serde::Deserialize;
use serde::Serialize;
use serde::Serializer;
use starknet::core::types::ExecutionResources;
use starknet::core::types::{BlockId, Event, ExecutionResult, Felt};
use starknet::providers::{ProviderError, Url};
use starknet_api::block::BlockNumber;
use starknet_api::core::EntryPointSelector;
use starknet_api::core::EthAddress;
use starknet_api::core::{ChainId, ContractAddress, Nonce};
use starknet_api::execution_resources::GasVector;
use starknet_api::transaction::fields::Calldata;
use starknet_api::transaction::fields::Fee;
use starknet_api::transaction::fields::PaymasterData;
use starknet_api::transaction::fields::TransactionSignature;
use starknet_api::transaction::fields::ValidResourceBounds;
use starknet_api::transaction::L1HandlerTransaction;
use starknet_api::transaction::L2ToL1Payload;
use starknet_api::transaction::MessageToL1;
use starknet_api::transaction::TransactionHash;
use starknet_api::transaction::TransactionVersion;
use starknet_api::StarknetApiError;
use starknet_types_core::felt::FromStrError;
use std::collections::HashMap;
use std::sync::Arc;
use thiserror::Error;
use tracing::error;
use utils::parse_chain_id_and_rpc_url_debug;
use utils::parse_optional_tx_hash;
use utils::parse_transaction_type;
use utils::{
    eth_address_to_felt, eth_u256_to_felt, parse_block_number, parse_calldata,
    parse_chain_id_and_rpc_url, parse_contract_address, parse_nonce, parse_transaction_version,
};
use walnut_shared::Parameter;

#[derive(Serialize, Deserialize, Debug, Clone)]
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
    pub entry_point_selector: Option<EntryPointSelector>,
    pub calldata: Calldata,
    pub transaction_version: TransactionVersion,
    pub transaction_signature: Option<TransactionSignature>,
    pub transaction_hash: Option<TransactionHash>,
    pub transaction_type: Option<TransactionType>,
    pub max_fee: Option<Fee>,
    pub resource_bounds: Option<ValidResourceBounds>,
    pub paymaster_data: Option<PaymasterData>,
    pub strkgate_event: Option<Event>,
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
            entry_point_selector: None,
            calldata,
            transaction_version,
            transaction_signature: None,
            transaction_hash: None,
            transaction_type: Some(TransactionType::InvokeFunction),
            max_fee: None,
            resource_bounds: None,
            paymaster_data: None,
            strkgate_event: None,
        })
    }

    pub async fn try_from_debug_payload(
        raw: DebugPayload,
    ) -> Result<Self, TransactionSimulationError> {
        let (chain_id, rpc_url) = parse_chain_id_and_rpc_url_debug(&raw).await?;
        let nonce = parse_nonce(raw.nonce);
        let block_number = parse_block_number(raw.block_number);
        let sender_address = parse_contract_address(&raw.sender_address)?;
        let calldata = parse_calldata(&raw.calldata)?;
        let transaction_version = parse_transaction_version(raw.transaction_version)?;
        let transaction_hash = parse_optional_tx_hash(raw.transaction_hash.as_deref())?;
        let transaction_type = Some(parse_transaction_type(&raw.transaction_type)?);

        Ok(Self {
            chain_id,
            rpc_url,
            block_number,
            nonce,
            sender_address,
            entry_point_selector: None,
            calldata,
            transaction_signature: None,
            transaction_version,
            transaction_hash,
            transaction_type,
            max_fee: None,
            resource_bounds: None,
            paymaster_data: None,
            strkgate_event: None,
        })
    }
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct DebugPayload {
    pub chain_id: Option<String>,
    pub rpc_url: Option<String>,
    pub block_number: Option<u64>,
    pub block_timestamp: Option<u64>,
    pub nonce: Option<u64>,
    pub sender_address: String,
    pub calldata: Vec<String>,
    pub transaction_version: usize,
    pub transaction_type: String,
    pub transaction_index_in_block: Option<u64>,
    pub total_transactions_in_block: Option<u64>,
    pub transaction_hash: Option<String>,
}

#[derive(Serialize, Debug)]
pub struct DetailedTransactionReceipt {
    pub fee: Fee,
    pub estimated_fee: Option<String>,

    pub gas: GasVector,
    pub da_gas: GasVector,

    pub starknet_resources_gas_vector: GasVector,
    pub starknet_resources_archival_data_gas_vector: GasVector,
    pub starknet_resources_message_gas_vector: GasVector,
    pub starknet_resources_state_gas_vector: GasVector,

    pub computation_resources_gas_vector: GasVector,
    pub computation_resources_vm_cost_gas_vector: GasVector,
    pub computation_resources_sierra_gas_vector: GasVector,
}

#[derive(Serialize, Debug, Clone, Default)]
pub struct FlameChartNode {
    pub call_id: u32,
    pub raw_value: u64,
    pub value: f64,
    pub name: Option<String>,
    pub children: Vec<FlameChartNode>,
}

#[derive(Serialize, Debug)]
pub struct L1TransactionData {
    pub chain_id: String,
    pub block_number: Option<u64>,
    pub sender_address: String,
    pub receiver_address: Option<String>,
    pub transaction_type: Option<String>,
    pub status: Option<String>,
    pub message_hashes: Vec<String>,
    pub l1_tx_hash: Option<String>,
}

#[derive(Serialize, Debug)]
pub struct L2TransactionData {
    pub simulation_result: SimulationInfo,
    pub chain_id: String,
    #[serde(serialize_with = "serialize_block_number")]
    pub block_number: BlockId,
    pub block_timestamp: u64,
    pub nonce: Option<u64>,
    pub sender_address: String,
    pub calldata: Vec<String>,
    pub transaction_version: usize,
    pub transaction_type: String,
    pub transaction_index_in_block: Option<usize>,
    pub total_transactions_in_block: Option<usize>,
    pub l1_tx_hash: Option<String>,
    pub l2_tx_hash: Option<String>,
    pub flamechart: Option<FlameChartNode>,
    pub actual_fee: Option<String>,
    pub execution_resources: Option<ExecutionResources>,
}

#[derive(Serialize, Debug)]
pub struct TransactionSimulationResult {
    pub l1_transaction_data: Option<L1TransactionData>,
    pub l2_transaction_data: Option<L2TransactionData>,
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
    #[error("Transaction hash {0} not found on {1}")]
    TransactionHashNotFound(String, String),
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
    #[error("Fee check error")]
    FeeCheckError(#[from] FeeCheckError),
    #[error("Failed to convert to L1HandlerTransaction")]
    ConversionError,
    #[error("Trace Error occurred: {0}")]
    TraceError(String),
    #[error("Error occurred: {0}")]
    OtherError(String),
}

impl From<anyhow::Error> for TransactionSimulationError {
    fn from(err: anyhow::Error) -> Self {
        TransactionSimulationError::TraceError(err.to_string())
    }
}

fn serialize_block_number<S>(block_id: &BlockId, serializer: S) -> Result<S::Ok, S::Error>
where
    S: Serializer,
{
    match block_id {
        BlockId::Number(num) => serializer.serialize_u64(*num),
        BlockId::Hash(hash) => serializer.serialize_str(&format!("{:?}", hash)),
        BlockId::Tag(tag) => serializer.serialize_str(&format!("{:?}", tag)),
    }
}

#[derive(Serialize, Debug)]
pub struct DebuggerInfo {
    pub contract_calls_map: ContractCallsMap,
    pub function_calls_map: FunctionCallsMap,
    pub simulation_debugger_data: Option<SimulationDebuggerData>,
}

#[derive(Serialize, Debug)]
pub struct SimulationInfo {
    pub contract_calls_map: ContractCallsMap,
    pub function_calls_map: FunctionCallsMap,
    pub event_calls_map: EventCallsMap,
    pub events: Vec<EmittedEvent>,
    pub execution_result: ExecutionResult,
    pub simulation_debugger_data: Option<SimulationDebuggerData>,
    pub storage_changes: HashMap<u32, HashMap<String, (String, String)>>,
    pub estimated_fee: Option<String>,
}

#[derive(Serialize, Debug)]
pub struct ContractCallEvent {
    pub contract_call_id: u32,
    pub name: String,
    pub keys: Vec<String>,
    pub parameters: Vec<Parameter>,
    pub data: Vec<String>,
}

#[derive(Debug)]
pub struct DecodedL2ToL1Message {
    pub event: EStarknetL2L1Event,
    pub message_hash: String,
}

#[derive(Debug)]
pub enum EStarknetL2L1Event {
    LogMessageToL1 {
        from_address: String,
        to_address: String,
        payload: Vec<String>,
    },
    ConsumedMessageToL1 {
        from_address: U256,
        to_address: Address,
        payload: Vec<U256>,
    },
}

impl TryFrom<EStarknetL2L1Event> for MessageToL1 {
    type Error = StarknetApiError;

    fn try_from(message: EStarknetL2L1Event) -> Result<Self, Self::Error> {
        match message {
            EStarknetL2L1Event::ConsumedMessageToL1 {
                from_address,
                to_address,
                payload,
            } => {
                let from_felt = eth_u256_to_felt(from_address);
                let from_address = ContractAddress::try_from(from_felt)?;

                let payload_felts = payload.into_iter().map(eth_u256_to_felt).collect();

                Ok(MessageToL1 {
                    from_address,
                    to_address: EthAddress(to_address),
                    payload: L2ToL1Payload(payload_felts),
                })
            }
            _ => Err(StarknetApiError::InvalidResourceMappingInitializer(
                "Invalid event variant".to_string(),
            )),
        }
    }
}

#[derive(Debug, Clone)]
pub enum EStarknetL1L2Event {
    LogMessageToL2 {
        from_address: Address,
        to_address: U256,
        selector: U256,
        payload: Vec<U256>,
        nonce: U256,
        fee: U256,
    },
    ConsumedMessageToL2 {
        from_address: Address,
        to_address: U256,
        selector: U256,
        payload: Vec<U256>,
        nonce: U256,
    },
}

impl TryFrom<EStarknetL1L2Event> for L1HandlerTransaction {
    type Error = StarknetApiError;

    fn try_from(event: EStarknetL1L2Event) -> Result<Self, Self::Error> {
        match event {
            EStarknetL1L2Event::LogMessageToL2 {
                from_address,
                to_address,
                selector,
                payload,
                nonce,
                fee: _,
            } => {
                let sender_felt = eth_address_to_felt(from_address);

                let mut calldata_vec = vec![sender_felt];
                for p in payload {
                    let felt = eth_u256_to_felt(p);
                    calldata_vec.push(felt);
                }
                let calldata = Calldata(Arc::new(calldata_vec));

                let contract_address_to_felt = eth_u256_to_felt(to_address);
                let contract_address = ContractAddress::try_from(contract_address_to_felt)?;

                let selector_felt = eth_u256_to_felt(selector);
                let entry_point_selector = EntryPointSelector(selector_felt);

                let nonce_felt = eth_u256_to_felt(nonce);
                let nonce = Nonce(nonce_felt);

                Ok(L1HandlerTransaction {
                    version: starknet_api::transaction::TransactionVersion::ONE,
                    nonce,
                    contract_address,
                    entry_point_selector,
                    calldata,
                })
            }
            _ => Err(StarknetApiError::InvalidResourceMappingInitializer(
                "Invalid event variant".to_string(),
            )),
        }
    }
}
