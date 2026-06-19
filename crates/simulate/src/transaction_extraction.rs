use blockifier::blockifier_versioned_constants::VersionedConstants;
use blockifier::bouncer::BouncerConfig;
use blockifier::context::TransactionContext;
use blockifier::context::{BlockContext, ChainInfo, FeeTokenAddresses};
use blockifier::transaction::objects::{
    CommonAccountFields, CurrentTransactionInfo, DeprecatedTransactionInfo, TransactionInfo,
};
use num_traits::ToPrimitive;
use starknet_api::block::BlockNumber;
use starknet_api::block::BlockTimestamp;
use starknet_api::block::{BlockInfo, GasPriceVector, GasPrices, NonzeroGasPrice, StarknetVersion};
use starknet_api::contract_address;
use starknet_api::core::{ChainId, ContractAddress, EntryPointSelector, Nonce};
use starknet_api::data_availability::DataAvailabilityMode;
use starknet_api::executable_transaction::TransactionType;
use starknet_api::transaction::fields::{
    Calldata, Fee, PaymasterData, ProofFacts, TransactionSignature, ValidResourceBounds,
};
use starknet_api::transaction::TransactionVersion;
use starknet_rust::core::types::{
    BlockId, BlockWithTxs, DeclareTransaction, Event, ExecutionResources, ExecutionResult, Felt,
    InvokeTransaction, MaybePreConfirmedBlockWithTxs, Transaction, TransactionReceipt,
};
use starknet_rust::providers::{
    jsonrpc::{HttpTransport, JsonRpcClient},
    Provider,
};
use std::sync::Arc;
use walnut_shared::{felts_to_string, format_fee_payment};
use walnut_shared::{
    resource_bounds_mapping_to_default_valid_resource_bounds,
    resource_bounds_mapping_to_valid_resource_bounds,
};
use walnut_shared::{ETH_FEE_TOKEN_ADDRESS, STRK_FEE_TOKEN_ADDRESS};

use crate::transaction_info::TransactionInformation;
use crate::SimulationArgs;
use crate::TransactionSimulationError;

// Fields pulled out of a submitted (account) transaction.
type SubmittedTxFields = (
    Nonce,
    ContractAddress,
    EntryPointSelector,
    Calldata,
    TransactionVersion,
    TransactionType,
    TransactionSignature,
    Fee,
    ValidResourceBounds,
    PaymasterData,
);

// TODO: Refactor this one
pub fn extract_submitted_tx(transaction: Transaction) -> Option<SubmittedTxFields> {
    match transaction {
        Transaction::Invoke(invoke_transaction) => match invoke_transaction {
            InvokeTransaction::V0(tx) => Some((
                Nonce::default(),
                ContractAddress::try_from(tx.contract_address).unwrap_or_default(),
                EntryPointSelector::default(),
                Calldata(tx.calldata.into()),
                TransactionVersion::ZERO,
                TransactionType::InvokeFunction,
                TransactionSignature(tx.signature.into()),
                Fee(tx.max_fee.to_u128().unwrap_or_default()),
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
                TransactionSignature(tx.signature.into()),
                Fee(tx.max_fee.to_u128().unwrap_or_default()),
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
                TransactionSignature(tx.signature.into()),
                Fee(u128::MAX),
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
                TransactionSignature(tx.signature.into()),
                Fee(tx.max_fee.to_u128().unwrap_or_default()),
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
                TransactionSignature(tx.signature.into()),
                Fee(tx.max_fee.to_u128().unwrap_or_default()),
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
                TransactionSignature(tx.signature.into()),
                Fee(tx.max_fee.to_u128().unwrap_or_default()),
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
                TransactionSignature(tx.signature.into()),
                Fee::default(),
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
            Fee::default(),
            resource_bounds_mapping_to_default_valid_resource_bounds(),
            PaymasterData::default(),
        )),
        Transaction::Deploy(_) | Transaction::DeployAccount(_) => {
            // DEPLOY and DEPLOY_ACCOUNT transactions are currently not supported
            None
        }
    }
}

pub fn extract_transaction_signature(transaction: Transaction) -> Option<TransactionSignature> {
    match transaction {
        Transaction::Invoke(invoke_tx) => match invoke_tx {
            InvokeTransaction::V0(tx) => Some(TransactionSignature(tx.signature.into())),
            InvokeTransaction::V1(tx) => Some(TransactionSignature(tx.signature.into())),
            InvokeTransaction::V3(tx) => Some(TransactionSignature(tx.signature.into())),
        },
        Transaction::Declare(declare_tx) => match declare_tx {
            DeclareTransaction::V0(tx) => Some(TransactionSignature(tx.signature.into())),
            DeclareTransaction::V1(tx) => Some(TransactionSignature(tx.signature.into())),
            DeclareTransaction::V2(tx) => Some(TransactionSignature(tx.signature.into())),
            DeclareTransaction::V3(tx) => Some(TransactionSignature(tx.signature.into())),
        },
        // L1Handler transactions have no signature
        Transaction::L1Handler(_) => Some(TransactionSignature::default()),
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

pub fn extract_actual_fee_transaction_receipt(
    transaction_receipt: &TransactionReceipt,
) -> Option<String> {
    match transaction_receipt {
        TransactionReceipt::Invoke(receipt) => format_fee_payment(&receipt.actual_fee),
        TransactionReceipt::Declare(receipt) => format_fee_payment(&receipt.actual_fee),
        TransactionReceipt::L1Handler(receipt) => format_fee_payment(&receipt.actual_fee),
        _ => None,
    }
}

pub fn extract_execution_resources_transaction_receipt(
    transaction_receipt: &TransactionReceipt,
) -> Option<ExecutionResources> {
    match transaction_receipt {
        TransactionReceipt::Invoke(receipt) => Some(receipt.execution_resources.clone()),
        TransactionReceipt::Declare(receipt) => Some(receipt.execution_resources.clone()),
        TransactionReceipt::L1Handler(receipt) => Some(receipt.execution_resources.clone()),
        _ => None,
    }
}

async fn fetch_block_with_txs(
    provider_client: &JsonRpcClient<HttpTransport>,
    block_number: u64,
) -> Result<BlockWithTxs, TransactionSimulationError> {
    let block_id = BlockId::Number(block_number);
    let block_with_txs = provider_client.get_block_with_txs(block_id, None).await;

    match block_with_txs {
        Ok(MaybePreConfirmedBlockWithTxs::Block(block_txs)) => Ok(block_txs),
        Ok(MaybePreConfirmedBlockWithTxs::PreConfirmedBlock(_)) => {
            Err(TransactionSimulationError::PreConfirmedBlock(
                "Pre-confirmed block is not allowed at the configuration level".to_string(),
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
    provider_client: &starknet_rust::providers::jsonrpc::JsonRpcClient<
        starknet_rust::providers::jsonrpc::HttpTransport,
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
        // We always run with the latest versioned constants (see extract_transaction_contex),
        // so keep the block's starknet version consistent with that.
        starknet_version: StarknetVersion::LATEST,
        sequencer_address: ContractAddress::try_from(block_txs.sequencer_address)
            .unwrap_or_default(),
        block_timestamp: BlockTimestamp(block_txs.timestamp),
        gas_prices,
        // A field which indicates if EIP-4844 blobs are used for publishing state diffs to l1
        // This has influence on the cost of publishing the data on l1
        use_kzg_da: true,
    };
    let total_txs_in_block = block_txs.transactions.len();
    let transaction_index = extract_transaction_index(&block_txs, simulation_args);
    Ok((block_info, transaction_index, total_txs_in_block))
}

pub fn extract_transaction_index(
    block_with_txs: &starknet_rust::core::types::BlockWithTxs,
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
    args: &SimulationArgs,
    block_info: &BlockInfo,
) -> Arc<TransactionContext> {
    let chain_id = args.chain_id.clone();
    let transaction_version = args.transaction_version;
    let transaction_type = args.transaction_type;

    let chain_info = ChainInfo {
        chain_id,
        fee_token_addresses: FeeTokenAddresses {
            strk_fee_token_address: contract_address!(STRK_FEE_TOKEN_ADDRESS),
            eth_fee_token_address: contract_address!(ETH_FEE_TOKEN_ADDRESS),
        },
        is_l3: false,
    };

    let transaction_info = match (transaction_version, transaction_type.as_ref()) {
        (_, Some(TransactionType::L1Handler)) => create_deprecated_info(args, None),
        (version, Some(TransactionType::InvokeFunction))
            if version.0 == Felt::ZERO || version.0 == Felt::ONE =>
        {
            create_deprecated_info(args, Some(Fee(u128::MAX)))
        }
        _ => create_current_info(args),
    };

    Arc::new(TransactionContext {
        block_context: Arc::new(BlockContext::new(
            block_info.clone(),
            chain_info,
            VersionedConstants::get_versioned_constants(None),
            BouncerConfig::default(),
        )),
        tx_info: transaction_info,
    })
}

fn create_deprecated_info(args: &SimulationArgs, default_max_fee: Option<Fee>) -> TransactionInfo {
    TransactionInfo::Deprecated(DeprecatedTransactionInfo {
        common_fields: CommonAccountFields {
            transaction_hash: args.transaction_hash.unwrap_or_default(),
            version: args.transaction_version,
            signature: args.transaction_signature.clone().unwrap_or_default(),
            nonce: args.nonce.unwrap_or_default(),
            sender_address: args.sender_address,
            only_query: false,
        },
        max_fee: args.max_fee.or(default_max_fee).unwrap_or_default(),
    })
}

fn create_current_info(args: &SimulationArgs) -> TransactionInfo {
    TransactionInfo::Current(CurrentTransactionInfo {
        common_fields: CommonAccountFields {
            transaction_hash: args.transaction_hash.unwrap_or_default(),
            version: args.transaction_version,
            signature: args.transaction_signature.clone().unwrap_or_default(),
            nonce: args.nonce.unwrap_or_default(),
            sender_address: args.sender_address,
            only_query: false,
        },
        resource_bounds: args
            .resource_bounds
            .unwrap_or_else(resource_bounds_mapping_to_default_valid_resource_bounds),
        tip: Default::default(),
        nonce_data_availability_mode: DataAvailabilityMode::L1,
        fee_data_availability_mode: DataAvailabilityMode::L1,
        paymaster_data: args.paymaster_data.clone().unwrap_or_default(),
        account_deployment_data: Default::default(),
        proof_facts: ProofFacts::default(),
    })
}

pub fn extract_chain_id_from_felt(
    chain_id_felt: Felt,
) -> Result<ChainId, TransactionSimulationError> {
    let chain_id_string = felts_to_string(&[chain_id_felt]);
    Ok(ChainId::Other(chain_id_string))
}
