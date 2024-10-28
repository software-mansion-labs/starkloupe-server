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
use starknet::core::types::Felt;
use starknet_api::block::BlockTimestamp;
use std::sync::Arc;

use starknet_api::core::{ChainId, ContractAddress, Nonce, PatriciaKey};
use starknet_api::data_availability::DataAvailabilityMode;
use starknet_api::transaction::{
    Calldata, ResourceBoundsMapping, TransactionHash, TransactionSignature, TransactionVersion,
};
use starknet_api::{contract_address, felt, patricia_key};
use walnut_shared::{ETH_FEE_TOKEN_ADDRESS, STRK_FEE_TOKEN_ADDRESS};

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

// TODO: Find a better way to do this
fn match_transaction(tx: &starknet_old_types::Transaction, args: &SimulationArgs) -> bool {
    let sender_address = Felt::from(*args.sender_address.0);
    let nonce = args.nonce.as_ref().map(|n| Felt::from(n.0));
    match tx {
        starknet_old_types::Transaction::Invoke(invoke_tx) => {
            match (invoke_tx, args.transaction_version.0) {
                (starknet_old_types::InvokeTransaction::V0(tx_v0), version)
                    if version == Felt::ZERO =>
                {
                    sender_address == field_element_to_felt(tx_v0.contract_address)
                        && args.calldata == vec_field_element_to_vec_felt(tx_v0.calldata.clone())
                }
                (starknet_old_types::InvokeTransaction::V1(tx_v1), version)
                    if version == Felt::ONE =>
                {
                    sender_address == field_element_to_felt(tx_v1.sender_address)
                        && args.calldata == vec_field_element_to_vec_felt(tx_v1.calldata.clone())
                        && nonce
                            .as_ref()
                            .map_or(false, |n| *n == field_element_to_felt(tx_v1.nonce))
                }
                (starknet_old_types::InvokeTransaction::V3(tx_v3), version)
                    if version == Felt::THREE =>
                {
                    sender_address == field_element_to_felt(tx_v3.sender_address)
                        && args.calldata == vec_field_element_to_vec_felt(tx_v3.calldata.clone())
                        && nonce
                            .as_ref()
                            .map_or(false, |n| *n == field_element_to_felt(tx_v3.nonce))
                }
                _ => false,
            }
        }
        starknet_old_types::Transaction::L1Handler(l1_handler_tx) => {
            let version: Felt = args.transaction_version.0;
            let l1_hanler_version: Felt = field_element_to_felt(l1_handler_tx.version);
            let _l1_handler_nonce: Felt = Felt::from(l1_handler_tx.nonce);
            version == l1_hanler_version
                && sender_address == field_element_to_felt(l1_handler_tx.contract_address)
                && args.calldata == vec_field_element_to_vec_felt(l1_handler_tx.calldata.clone())
                && nonce
                    .as_ref()
                    .map_or(false, |n| *n == Felt::from(l1_handler_tx.nonce))
        }
        starknet_old_types::Transaction::Declare(declare_tx) => {
            match (declare_tx, args.transaction_version.0) {
                (starknet_old_types::DeclareTransaction::V0(tx_v0), version)
                    if version == Felt::ZERO =>
                {
                    sender_address == field_element_to_felt(tx_v0.sender_address)
                }
                (starknet_old_types::DeclareTransaction::V1(tx_v1), version)
                    if version == Felt::ONE =>
                {
                    sender_address == field_element_to_felt(tx_v1.sender_address)
                        && nonce
                            .as_ref()
                            .map_or(false, |n| *n == field_element_to_felt(tx_v1.nonce))
                }
                (starknet_old_types::DeclareTransaction::V2(tx_v2), version)
                    if version == Felt::TWO =>
                {
                    sender_address == field_element_to_felt(tx_v2.sender_address)
                        && nonce
                            .as_ref()
                            .map_or(false, |n| *n == field_element_to_felt(tx_v2.nonce))
                }
                (starknet_old_types::DeclareTransaction::V3(tx_v3), version)
                    if version == Felt::THREE =>
                {
                    sender_address == field_element_to_felt(tx_v3.sender_address)
                        && nonce
                            .as_ref()
                            .map_or(false, |n| *n == field_element_to_felt(tx_v3.nonce))
                }
                _ => false,
            }
        }
        starknet_old_types::Transaction::Deploy(deploy_tx) => {
            let version: Felt = args.transaction_version.0;
            let deploy_version: Felt = field_element_to_felt(deploy_tx.version);
            version == deploy_version
                && args.calldata
                    == vec_field_element_to_vec_felt(deploy_tx.constructor_calldata.clone())
        }
        starknet_old_types::Transaction::DeployAccount(deploy_account_tx) => {
            match deploy_account_tx {
                starknet_old_types::DeployAccountTransaction::V1(tx_v1) => {
                    args.calldata
                        == vec_field_element_to_vec_felt(tx_v1.constructor_calldata.clone())
                        && nonce
                            .as_ref()
                            .map_or(false, |n| *n == field_element_to_felt(tx_v1.nonce))
                }
                starknet_old_types::DeployAccountTransaction::V3(tx_v3) => {
                    args.calldata
                        == vec_field_element_to_vec_felt(tx_v3.constructor_calldata.clone())
                        && nonce
                            .as_ref()
                            .map_or(false, |n| *n == field_element_to_felt(tx_v3.nonce))
                }
            }
        }
    }
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
