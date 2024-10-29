use starknet_old::core::types as starknet_old_types;
use starknet_providers::jsonrpc::HttpTransport;
use starknet_providers::JsonRpcClient;
use starknet_providers::Provider;
use walnut_shared::field_element_to_felt;
use walnut_shared::vec_field_element_to_vec_felt;

use blockifier::blockifier::block::BlockInfo;
use blockifier::bouncer::BouncerConfig;
use blockifier::context::TransactionContext;
use blockifier::context::{BlockContext, ChainInfo, FeeTokenAddresses};
use blockifier::transaction::objects::{
    CommonAccountFields, CurrentTransactionInfo, TransactionInfo,
};
use blockifier::transaction::transaction_types::TransactionType;
use blockifier::versioned_constants::VersionedConstants;
use starknet_api::block::BlockTimestamp;
use std::sync::Arc;

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
            )),
            starknet_old_types::InvokeTransaction::V1(tx) => Some((
                Nonce(field_element_to_felt(tx.nonce)),
                field_element_to_felt(tx.sender_address).try_into().unwrap(),
                Calldata(vec_field_element_to_vec_felt(tx.calldata).into()),
                TransactionVersion::ONE,
                TransactionType::InvokeFunction,
                TransactionSignature(vec_field_element_to_vec_felt(tx.signature).into()),
            )),
            starknet_old_types::InvokeTransaction::V3(tx) => Some((
                Nonce(field_element_to_felt(tx.nonce)),
                field_element_to_felt(tx.sender_address).try_into().unwrap(),
                Calldata(vec_field_element_to_vec_felt(tx.calldata).into()),
                TransactionVersion::THREE,
                TransactionType::InvokeFunction,
                TransactionSignature(vec_field_element_to_vec_felt(tx.signature).into()),
            )),
        },
        starknet_old_types::Transaction::Declare(declare_transaction) => {
            match declare_transaction {
                starknet_old_types::DeclareTransaction::V0(tx) => Some((
                    Nonce::default(),
                    field_element_to_felt(tx.sender_address).try_into().unwrap(),
                    Calldata::default(),
                    TransactionVersion::ZERO,
                    TransactionType::Declare,
                    TransactionSignature(vec_field_element_to_vec_felt(tx.signature).into()),
                )),
                starknet_old_types::DeclareTransaction::V1(tx) => Some((
                    Nonce(field_element_to_felt(tx.nonce)),
                    field_element_to_felt(tx.sender_address).try_into().unwrap(),
                    Calldata::default(),
                    TransactionVersion::ONE,
                    TransactionType::Declare,
                    TransactionSignature(vec_field_element_to_vec_felt(tx.signature).into()),
                )),
                starknet_old_types::DeclareTransaction::V2(tx) => Some((
                    Nonce(field_element_to_felt(tx.nonce)),
                    field_element_to_felt(tx.sender_address).try_into().unwrap(),
                    Calldata::default(),
                    TransactionVersion::TWO,
                    TransactionType::Declare,
                    TransactionSignature(vec_field_element_to_vec_felt(tx.signature).into()),
                )),
                starknet_old_types::DeclareTransaction::V3(tx) => Some((
                    Nonce(field_element_to_felt(tx.nonce)),
                    field_element_to_felt(tx.sender_address).try_into().unwrap(),
                    Calldata::default(),
                    TransactionVersion::THREE,
                    TransactionType::Declare,
                    TransactionSignature(vec_field_element_to_vec_felt(tx.signature).into()),
                )),
            }
        }
        _ => None,
    }
}

pub fn extract_transaction_receipt(
    transaction_receipt: starknet_old_types::MaybePendingTransactionReceipt,
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

pub async fn extract_block_txs_info(
    provider_client: &JsonRpcClient<HttpTransport>,
    simulation_args: &SimulationArgs,
    block_number: u64,
) -> Result<(BlockTimestamp, usize), TransactionSimulationError> {
    let block_id = starknet_old_types::BlockId::Number(block_number);
    let block_with_txs = provider_client.get_block_with_txs(block_id).await;
    match block_with_txs {
        Ok(starknet_old_types::MaybePendingBlockWithTxs::Block(block_txs)) => {
            let block_timestamp = BlockTimestamp(block_txs.timestamp);
            let transaction_index = extract_transaction_index(&block_txs, simulation_args);
            Ok((block_timestamp, transaction_index))
        }
        Ok(starknet_old_types::MaybePendingBlockWithTxs::PendingBlock(_)) => {
            Err(TransactionSimulationError::PendingBlock(
                "Pending block is not allowed at the configuration level".to_string(),
            ))
        }
        Err(err) => Err(TransactionSimulationError::ProviderError(err)),
    }
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
    transaction_signature: &Option<TransactionSignature>,
    transaction_hash: &Option<TransactionHash>,
    nonce: &Option<Nonce>,
    chain_id: Option<ChainId>,
    block_info: &BlockInfo,
) -> Arc<TransactionContext> {
    // Create a chain-specific block context
    let chain_info = if let Some(chain_id) = chain_id {
        ChainInfo {
            chain_id,
            fee_token_addresses: FeeTokenAddresses {
                strk_fee_token_address: contract_address!(STRK_FEE_TOKEN_ADDRESS),
                eth_fee_token_address: contract_address!(ETH_FEE_TOKEN_ADDRESS),
            },
        }
    } else {
        ChainInfo::default()
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
                signature: transaction_signature.clone().unwrap_or_default(),
                nonce: nonce.unwrap_or_default(),
                sender_address: *sender_address,
                only_query: false,
            },
            resource_bounds: ResourceBoundsMapping::default(),
            tip: Default::default(),
            nonce_data_availability_mode: DataAvailabilityMode::L1,
            fee_data_availability_mode: DataAvailabilityMode::L1,
            paymaster_data: Default::default(),
            account_deployment_data: Default::default(),
        }),
    })
}
