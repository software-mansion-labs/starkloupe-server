use blockifier::blockifier::block::BlockInfo;
use blockifier::bouncer::BouncerConfig;
use blockifier::context::TransactionContext;
use blockifier::context::{BlockContext, ChainInfo, FeeTokenAddresses};
use blockifier::transaction::objects::{
    CommonAccountFields, CurrentTransactionInfo, DeprecatedTransactionInfo, TransactionInfo,
};
use blockifier::transaction::transaction_types::TransactionType;
use blockifier::versioned_constants::VersionedConstants;
use runtime::starknet::context::SerializableGasPrices;
use starknet::core::types::Felt;
use starknet_api::block::BlockNumber;
use starknet_api::block::BlockTimestamp;
use starknet_api::transaction::PaymasterData;
use starknet_old::core::types::{self as starknet_old_types, Event};
use starknet_providers::jsonrpc::HttpTransport;
use starknet_providers::JsonRpcClient;
use starknet_providers::Provider;
use std::sync::Arc;
use walnut_shared::field_element_to_felt;
use walnut_shared::old_resource_bounds_mapping_to_resource_bounds_b_tree_map;
use walnut_shared::vec_field_element_to_vec_felt;
use walnut_shared::{felts_to_string, max_resource_bounds_map};

use starknet_api::core::{ChainId, ContractAddress, Nonce, PatriciaKey};
use starknet_api::data_availability::DataAvailabilityMode;
use starknet_api::transaction::{
    Calldata, ResourceBoundsMapping, TransactionHash, TransactionSignature, TransactionVersion,
};
use starknet_api::{contract_address, felt, patricia_key};
use walnut_shared::{ETH_FEE_TOKEN_ADDRESS, STRK_FEE_TOKEN_ADDRESS};

use crate::transaction_info::TransactionInformation;
use crate::SimulationArgs;
use crate::TransactionSimulationError;

pub fn extract_submitted_tx(
    transaction: starknet_old_types::Transaction,
) -> Option<(
    Nonce,
    ContractAddress,
    Calldata,
    TransactionVersion,
    TransactionType,
    TransactionSignature,
    ResourceBoundsMapping,
    PaymasterData,
)> {
    match transaction {
        starknet_old_types::Transaction::Invoke(invoke_transaction) => match invoke_transaction {
            starknet_old_types::InvokeTransaction::V0(tx) => Some((
                Nonce::default(),
                field_element_to_felt(tx.contract_address)
                    .try_into()
                    .unwrap(),
                Calldata(vec_field_element_to_vec_felt(tx.calldata).into()),
                TransactionVersion::ZERO,
                TransactionType::InvokeFunction,
                TransactionSignature(vec_field_element_to_vec_felt(tx.signature).into()),
                ResourceBoundsMapping::default(),
                PaymasterData::default(),
            )),
            starknet_old_types::InvokeTransaction::V1(tx) => Some((
                Nonce(field_element_to_felt(tx.nonce)),
                field_element_to_felt(tx.sender_address).try_into().unwrap(),
                Calldata(vec_field_element_to_vec_felt(tx.calldata).into()),
                TransactionVersion::ONE,
                TransactionType::InvokeFunction,
                TransactionSignature(vec_field_element_to_vec_felt(tx.signature).into()),
                ResourceBoundsMapping::default(),
                PaymasterData::default(),
            )),
            starknet_old_types::InvokeTransaction::V3(tx) => Some((
                Nonce(field_element_to_felt(tx.nonce)),
                field_element_to_felt(tx.sender_address).try_into().unwrap(),
                Calldata(vec_field_element_to_vec_felt(tx.calldata).into()),
                TransactionVersion::THREE,
                TransactionType::InvokeFunction,
                TransactionSignature(vec_field_element_to_vec_felt(tx.signature).into()),
                old_resource_bounds_mapping_to_resource_bounds_b_tree_map(&tx.resource_bounds),
                PaymasterData(vec_field_element_to_vec_felt(tx.paymaster_data)),
            )),
        },
        starknet_old_types::Transaction::Declare(declare_transaction) => {
            match declare_transaction {
                starknet_old_types::DeclareTransaction::V0(tx) => Some((
                    Nonce::default(),
                    field_element_to_felt(tx.sender_address).try_into().unwrap(),
                    Calldata(vec_field_element_to_vec_felt(vec![tx.class_hash]).into()),
                    TransactionVersion::ZERO,
                    TransactionType::Declare,
                    TransactionSignature(vec_field_element_to_vec_felt(tx.signature).into()),
                    ResourceBoundsMapping::default(),
                    PaymasterData::default(),
                )),
                starknet_old_types::DeclareTransaction::V1(tx) => Some((
                    Nonce(field_element_to_felt(tx.nonce)),
                    field_element_to_felt(tx.sender_address).try_into().unwrap(),
                    Calldata(vec_field_element_to_vec_felt(vec![tx.class_hash]).into()),
                    TransactionVersion::ONE,
                    TransactionType::Declare,
                    TransactionSignature(vec_field_element_to_vec_felt(tx.signature).into()),
                    ResourceBoundsMapping::default(),
                    PaymasterData::default(),
                )),
                starknet_old_types::DeclareTransaction::V2(tx) => Some((
                    Nonce(field_element_to_felt(tx.nonce)),
                    field_element_to_felt(tx.sender_address).try_into().unwrap(),
                    Calldata(vec_field_element_to_vec_felt(vec![tx.class_hash]).into()),
                    TransactionVersion::TWO,
                    TransactionType::Declare,
                    TransactionSignature(vec_field_element_to_vec_felt(tx.signature).into()),
                    ResourceBoundsMapping::default(),
                    PaymasterData::default(),
                )),
                starknet_old_types::DeclareTransaction::V3(tx) => Some((
                    Nonce(field_element_to_felt(tx.nonce)),
                    field_element_to_felt(tx.sender_address).try_into().unwrap(),
                    Calldata(vec_field_element_to_vec_felt(vec![tx.class_hash]).into()),
                    TransactionVersion::THREE,
                    TransactionType::Declare,
                    TransactionSignature(vec_field_element_to_vec_felt(tx.signature).into()),
                    old_resource_bounds_mapping_to_resource_bounds_b_tree_map(&tx.resource_bounds),
                    PaymasterData(vec_field_element_to_vec_felt(tx.paymaster_data)),
                )),
            }
        }
        _ => None,
    }
}

pub fn extract_block_number_transaction_receipt(
    transaction_receipt: &starknet_old_types::MaybePendingTransactionReceipt,
) -> Option<u64> {
    match transaction_receipt {
        starknet_old_types::MaybePendingTransactionReceipt::Receipt(receipt) => match receipt {
            starknet_old_types::TransactionReceipt::Invoke(invoke_receipt) => {
                Some(invoke_receipt.block_number)
            }
            starknet_old_types::TransactionReceipt::Declare(declare_receipt) => {
                Some(declare_receipt.block_number)
            }
            _ => None,
        },
        _ => None,
    }
}

pub fn extract_execution_status_transaction_receipt(
    transaction_receipt: &starknet_old_types::MaybePendingTransactionReceipt,
) -> Option<starknet_old_types::ExecutionResult> {
    match transaction_receipt {
        starknet_old_types::MaybePendingTransactionReceipt::Receipt(receipt) => match receipt {
            starknet_old_types::TransactionReceipt::Invoke(invoke_receipt) => {
                Some(invoke_receipt.execution_result.clone())
            }
            starknet_old_types::TransactionReceipt::Declare(declare_receipt) => {
                Some(declare_receipt.execution_result.clone())
            }
            _ => None,
        },
        _ => None,
    }
}

pub fn extract_starkgate_event_transaction_receipt(
    transaction_receipt: &starknet_old_types::MaybePendingTransactionReceipt,
) -> Option<Event> {
    match transaction_receipt {
        starknet_old_types::MaybePendingTransactionReceipt::Receipt(receipt) => match receipt {
            starknet_old_types::TransactionReceipt::Invoke(invoke_receipt) => {
                invoke_receipt.events.last().cloned()
            }
            starknet_old_types::TransactionReceipt::Declare(declare_receipt) => {
                declare_receipt.events.last().cloned()
            }
            _ => None,
        },
        _ => None,
    }
}

async fn fetch_block_with_txs(
    provider_client: &JsonRpcClient<HttpTransport>,
    block_number: u64,
) -> Result<starknet_old_types::BlockWithTxs, TransactionSimulationError> {
    let block_id = starknet_old_types::BlockId::Number(block_number);
    let block_with_txs = provider_client.get_block_with_txs(block_id).await;

    match block_with_txs {
        Ok(starknet_old_types::MaybePendingBlockWithTxs::Block(block_txs)) => Ok(block_txs),
        Ok(starknet_old_types::MaybePendingBlockWithTxs::PendingBlock(_)) => {
            Err(TransactionSimulationError::PendingBlock(
                "Pending block is not allowed at the configuration level".to_string(),
            ))
        }
        Err(err) => Err(TransactionSimulationError::ProviderError(err)),
    }
}

pub async fn extract_block_timestamp(
    provider_client: &JsonRpcClient<HttpTransport>,
    block_number: u64,
) -> Result<BlockTimestamp, TransactionSimulationError> {
    let block_txs = fetch_block_with_txs(provider_client, block_number).await?;
    Ok(BlockTimestamp(block_txs.timestamp))
}

pub async fn extract_block_txs_info(
    provider_client: &JsonRpcClient<HttpTransport>,
    simulation_args: &SimulationArgs,
    block_number: u64,
) -> Result<(BlockInfo, usize, usize), TransactionSimulationError> {
    let block_txs = fetch_block_with_txs(provider_client, block_number).await?;
    let block_info = BlockInfo {
        block_number: BlockNumber(block_txs.block_number),
        sequencer_address: field_element_to_felt(block_txs.sequencer_address)
            .try_into()
            .unwrap(),
        block_timestamp: BlockTimestamp(block_txs.timestamp),
        gas_prices: SerializableGasPrices::default().into(),
        use_kzg_da: true,
    };
    let total_txs_in_block = block_txs.transactions.len();
    let transaction_index = extract_transaction_index(&block_txs, simulation_args);
    Ok((block_info, transaction_index, total_txs_in_block))
}

pub fn extract_transaction_index(
    block_with_txs: &starknet_old_types::BlockWithTxs,
    simulation_args: &SimulationArgs,
) -> usize {
    for (index, tx) in block_with_txs.transactions.iter().enumerate() {
        if match_transaction(tx, simulation_args) {
            return index;
        }
    }
    0
}

fn match_transaction(tx: &starknet_old_types::Transaction, args: &SimulationArgs) -> bool {
    let sender_address = *args.sender_address.0;
    let nonce = args.nonce.as_ref().map(|n| n.0);
    let version = args.transaction_version.0;

    if tx.version() != version {
        return false;
    }

    if let Some(tx_sender_address) = tx.sender_address() {
        if tx_sender_address != sender_address {
            return false;
        }
    }

    if let Some(arg_nonce) = nonce {
        if let Some(tx_nonce) = tx.nonce() {
            if tx_nonce != arg_nonce {
                return false;
            }
        } else {
            return false;
        }
    }

    if !args.calldata.0.is_empty() {
        if let Some(tx_calldata) = tx.calldata() {
            if args.calldata.0.to_vec() != tx_calldata {
                return false;
            }
        } else {
            return false;
        }
    }

    true
}

pub fn extract_transaction_contex(
    sender_address: &ContractAddress,
    transaction_version: &TransactionVersion,
    transaction_signature: Option<TransactionSignature>,
    transaction_hash: &Option<TransactionHash>,
    nonce: &Option<Nonce>,
    chain_id: ChainId,
    block_info: &BlockInfo,
    resource_bounds: Option<ResourceBoundsMapping>,
    paymaster_data: Option<PaymasterData>,
) -> Arc<TransactionContext> {
    // Create a chain-specific block context
    let chain_info = ChainInfo {
        chain_id,
        fee_token_addresses: FeeTokenAddresses {
            strk_fee_token_address: contract_address!(STRK_FEE_TOKEN_ADDRESS),
            eth_fee_token_address: contract_address!(ETH_FEE_TOKEN_ADDRESS),
        },
    };

    Arc::new(TransactionContext {
        block_context: BlockContext::new(
            block_info.clone(),
            chain_info,
            VersionedConstants::latest_constants().clone(),
            BouncerConfig::default(),
        ),
        tx_info: TransactionInfo::Current(CurrentTransactionInfo {
            common_fields: CommonAccountFields {
                transaction_hash: transaction_hash.unwrap_or_default(),
                version: *transaction_version,
                signature: transaction_signature.unwrap_or_default(),
                nonce: nonce.unwrap_or_default(),
                sender_address: *sender_address,
                only_query: false,
            },
            resource_bounds: resource_bounds.unwrap_or_else(max_resource_bounds_map),
            tip: Default::default(),
            nonce_data_availability_mode: DataAvailabilityMode::L1,
            fee_data_availability_mode: DataAvailabilityMode::L1,
            paymaster_data: paymaster_data.unwrap_or_default(),
            account_deployment_data: Default::default(),
        }),
    })
}

pub fn extract_chain_id_from_felt(
    chain_id_felt: Felt,
) -> Result<ChainId, TransactionSimulationError> {
    let chain_id_string = felts_to_string(&[chain_id_felt])
        .first()
        .cloned()
        .unwrap_or_default();
    Ok(ChainId::Other(chain_id_string))
}
