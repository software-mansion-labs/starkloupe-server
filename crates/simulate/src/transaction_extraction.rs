use blockifier::bouncer::BouncerConfig;
use blockifier::context::TransactionContext;
use blockifier::context::{BlockContext, ChainInfo, FeeTokenAddresses};
use blockifier::transaction::objects::{
    CommonAccountFields, CurrentTransactionInfo, DeprecatedTransactionInfo, TransactionInfo,
};
use blockifier::transaction::transaction_types::TransactionType;
use blockifier::versioned_constants::VersionedConstants;
use starknet::core::types::{
    BlockId, BlockWithTxs, DeclareTransaction, Event, ExecutionResult, Felt, InvokeTransaction,
    MaybePendingBlockWithTxs, Transaction, TransactionReceipt,
};
use starknet::providers::{
    jsonrpc::{HttpTransport, JsonRpcClient},
    Provider,
};
use starknet_api::block::BlockNumber;
use starknet_api::block::BlockTimestamp;
use starknet_api::block::{BlockInfo, GasPriceVector, GasPrices, NonzeroGasPrice};
use starknet_api::contract_address;
use starknet_api::core::{ChainId, ContractAddress, EntryPointSelector, Nonce};
use starknet_api::data_availability::DataAvailabilityMode;
use starknet_api::transaction::fields::{
    Calldata, Fee, PaymasterData, TransactionSignature, ValidResourceBounds,
};
use starknet_api::transaction::{TransactionHash, TransactionVersion};
use std::sync::Arc;
use walnut_shared::felts_to_string;
use walnut_shared::{
    resource_bounds_mapping_to_default_valid_resource_bounds,
    resource_bounds_mapping_to_valid_resource_bounds,
};
use walnut_shared::{ETH_FEE_TOKEN_ADDRESS, STRK_FEE_TOKEN_ADDRESS};

use crate::transaction_info::TransactionInformation;
use crate::SimulationArgs;
use crate::TransactionSimulationError;

// TODO: Refactor this one
pub fn extract_submitted_tx(
    transaction: Transaction,
) -> Option<(
    Nonce,
    ContractAddress,
    EntryPointSelector,
    Calldata,
    TransactionVersion,
    TransactionType,
    TransactionSignature,
    ValidResourceBounds,
    PaymasterData,
)> {
    match transaction {
        Transaction::Invoke(invoke_transaction) => match invoke_transaction {
            InvokeTransaction::V0(tx) => Some((
                Nonce::default(),
                ContractAddress::try_from(tx.contract_address).unwrap_or_default(),
                EntryPointSelector::default(),
                Calldata(tx.calldata.into()),
                TransactionVersion::ZERO,
                TransactionType::InvokeFunction,
                TransactionSignature(tx.signature),
                resource_bounds_mapping_to_default_valid_resource_bounds(),
                PaymasterData::default(),
            )),
            InvokeTransaction::V1(tx) => Some((
                Nonce(tx.nonce),
                ContractAddress::try_from(tx.sender_address).unwrap_or_default(),
                EntryPointSelector::default(),
                Calldata(tx.calldata.into()),
                TransactionVersion::ONE,
                TransactionType::InvokeFunction,
                TransactionSignature(tx.signature),
                resource_bounds_mapping_to_default_valid_resource_bounds(),
                PaymasterData::default(),
            )),
            InvokeTransaction::V3(tx) => Some((
                Nonce(tx.nonce),
                ContractAddress::try_from(tx.sender_address).unwrap_or_default(),
                EntryPointSelector::default(),
                Calldata(tx.calldata.into()),
                TransactionVersion::THREE,
                TransactionType::InvokeFunction,
                TransactionSignature(tx.signature),
                resource_bounds_mapping_to_valid_resource_bounds(&tx.resource_bounds),
                PaymasterData(tx.paymaster_data),
            )),
        },
        Transaction::Declare(declare_transaction) => match declare_transaction {
            DeclareTransaction::V0(tx) => Some((
                Nonce::default(),
                ContractAddress::try_from(tx.sender_address).unwrap_or_default(),
                EntryPointSelector::default(),
                Calldata(vec![tx.class_hash].into()),
                TransactionVersion::ZERO,
                TransactionType::Declare,
                TransactionSignature(tx.signature),
                resource_bounds_mapping_to_default_valid_resource_bounds(),
                PaymasterData::default(),
            )),
            DeclareTransaction::V1(tx) => Some((
                Nonce(tx.nonce),
                ContractAddress::try_from(tx.sender_address).unwrap_or_default(),
                EntryPointSelector::default(),
                Calldata(vec![tx.class_hash].into()),
                TransactionVersion::ONE,
                TransactionType::Declare,
                TransactionSignature(tx.signature),
                resource_bounds_mapping_to_default_valid_resource_bounds(),
                PaymasterData::default(),
            )),
            DeclareTransaction::V2(tx) => Some((
                Nonce(tx.nonce),
                ContractAddress::try_from(tx.sender_address).unwrap_or_default(),
                EntryPointSelector::default(),
                Calldata(vec![tx.class_hash].into()),
                TransactionVersion::TWO,
                TransactionType::Declare,
                TransactionSignature(tx.signature),
                resource_bounds_mapping_to_default_valid_resource_bounds(),
                PaymasterData::default(),
            )),
            DeclareTransaction::V3(tx) => Some((
                Nonce(tx.nonce),
                ContractAddress::try_from(tx.sender_address).unwrap_or_default(),
                EntryPointSelector::default(),
                Calldata(vec![tx.class_hash].into()),
                TransactionVersion::THREE,
                TransactionType::Declare,
                TransactionSignature(tx.signature),
                resource_bounds_mapping_to_valid_resource_bounds(&tx.resource_bounds),
                PaymasterData(tx.paymaster_data),
            )),
        },
        Transaction::L1Handler(tx) => Some((
            Nonce(Felt::from(tx.nonce)),
            ContractAddress::try_from(tx.contract_address).unwrap_or_default(),
            EntryPointSelector(tx.entry_point_selector),
            Calldata(tx.calldata.into()),
            TransactionVersion::ZERO,
            TransactionType::L1Handler,
            TransactionSignature::default(),
            resource_bounds_mapping_to_default_valid_resource_bounds(),
            PaymasterData::default(),
        )),
        _ => None,
    }
}

pub fn extract_execution_status_transaction_receipt(
    transaction_receipt: &TransactionReceipt,
) -> Option<ExecutionResult> {
    match transaction_receipt {
        TransactionReceipt::Invoke(invoke_receipt) => Some(invoke_receipt.execution_result.clone()),
        TransactionReceipt::Declare(declare_receipt) => {
            Some(declare_receipt.execution_result.clone())
        }
        _ => None,
    }
}

pub fn extract_starkgate_event_transaction_receipt(
    transaction_receipt: &TransactionReceipt,
) -> Option<Event> {
    match transaction_receipt {
        TransactionReceipt::Invoke(receipt) => receipt.events.last().cloned(),
        TransactionReceipt::Declare(receipt) => receipt.events.last().cloned(),
        TransactionReceipt::L1Handler(receipt) => receipt.events.last().cloned(),
        _ => None,
    }
}

async fn fetch_block_with_txs(
    provider_client: &JsonRpcClient<HttpTransport>,
    block_number: u64,
) -> Result<BlockWithTxs, TransactionSimulationError> {
    let block_id = BlockId::Number(block_number);
    let block_with_txs = provider_client.get_block_with_txs(block_id).await;

    match block_with_txs {
        Ok(MaybePendingBlockWithTxs::Block(block_txs)) => Ok(block_txs),
        Ok(MaybePendingBlockWithTxs::PendingBlock(_)) => {
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
    provider_client: &starknet::providers::jsonrpc::JsonRpcClient<
        starknet::providers::jsonrpc::HttpTransport,
    >,
    simulation_args: &SimulationArgs,
    block_number: u64,
) -> Result<(BlockInfo, usize, usize), TransactionSimulationError> {
    let block_txs = fetch_block_with_txs(provider_client, block_number).await?;
    let gas_prices = GasPrices {
        eth_gas_prices: GasPriceVector {
            l1_gas_price: NonzeroGasPrice::try_from(
                u128::try_from(block_txs.l1_gas_price.price_in_wei).unwrap_or_default(),
            )
            .unwrap_or_default(),

            l1_data_gas_price: NonzeroGasPrice::try_from(
                u128::try_from(block_txs.l1_data_gas_price.price_in_wei).unwrap_or_default(),
            )
            .unwrap_or_default(),

            l2_gas_price: NonzeroGasPrice::try_from(
                u128::try_from(block_txs.l2_gas_price.price_in_wei).unwrap_or_default(),
            )
            .unwrap_or_default(),
        },

        strk_gas_prices: GasPriceVector {
            l1_gas_price: NonzeroGasPrice::try_from(
                u128::try_from(block_txs.l1_gas_price.price_in_fri).unwrap_or_default(),
            )
            .unwrap_or_default(),

            l1_data_gas_price: NonzeroGasPrice::try_from(
                u128::try_from(block_txs.l1_data_gas_price.price_in_fri).unwrap_or_default(),
            )
            .unwrap_or_default(),

            l2_gas_price: NonzeroGasPrice::try_from(
                u128::try_from(block_txs.l2_gas_price.price_in_fri).unwrap_or_default(),
            )
            .unwrap_or_default(),
        },
    };

    let block_info = BlockInfo {
        block_number: BlockNumber(block_txs.block_number),
        sequencer_address: ContractAddress::try_from(block_txs.sequencer_address)
            .unwrap_or_default(),
        block_timestamp: BlockTimestamp(block_txs.timestamp),
        gas_prices,
        use_kzg_da: true,
    };
    let total_txs_in_block = block_txs.transactions.len();
    let transaction_index = extract_transaction_index(&block_txs, simulation_args);
    Ok((block_info, transaction_index, total_txs_in_block))
}

pub fn extract_transaction_index(
    block_with_txs: &starknet::core::types::BlockWithTxs,
    simulation_args: &SimulationArgs,
) -> usize {
    for (index, tx) in block_with_txs.transactions.iter().enumerate() {
        if match_transaction(tx, simulation_args) {
            return index;
        }
    }
    0
}

fn match_transaction(tx: &Transaction, args: &SimulationArgs) -> bool {
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
    transaction_type: &Option<TransactionType>,
    nonce: &Option<Nonce>,
    chain_id: ChainId,
    block_info: &BlockInfo,
    resource_bounds: Option<ValidResourceBounds>,
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

    let transaction_info = match transaction_type {
        Some(TransactionType::L1Handler) => {
            TransactionInfo::Deprecated(DeprecatedTransactionInfo {
                common_fields: CommonAccountFields {
                    transaction_hash: transaction_hash.unwrap_or_default(),
                    version: *transaction_version,
                    signature: TransactionSignature::default(),
                    nonce: nonce.unwrap_or_default(),
                    sender_address: *sender_address,
                    only_query: false,
                },
                max_fee: Fee::default(),
            })
        }
        Some(_) | None => TransactionInfo::Current(CurrentTransactionInfo {
            common_fields: CommonAccountFields {
                transaction_hash: transaction_hash.unwrap_or_default(),
                version: *transaction_version,
                signature: transaction_signature.unwrap_or_default(),
                nonce: nonce.unwrap_or_default(),
                sender_address: *sender_address,
                only_query: false,
            },
            resource_bounds: resource_bounds
                .unwrap_or_else(|| resource_bounds_mapping_to_default_valid_resource_bounds()),
            tip: Default::default(),
            nonce_data_availability_mode: DataAvailabilityMode::L1,
            fee_data_availability_mode: DataAvailabilityMode::L1,
            paymaster_data: paymaster_data.unwrap_or_default(),
            account_deployment_data: Default::default(),
        }),
    };

    Arc::new(TransactionContext {
        block_context: BlockContext::new(
            block_info.clone(),
            chain_info,
            VersionedConstants::latest_constants().clone(),
            BouncerConfig::default(),
        ),
        tx_info: transaction_info,
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
