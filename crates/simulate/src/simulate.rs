use crate::contract_calls_map::ContractCallsMap;
use crate::contract_calls_map::ContractCallsMapBuilder;
use crate::contract_names::ContractNamesFetcher;
use crate::debugger_trace::DebuggerTraceBuilder;
use crate::events::EmittedEvent;
use crate::execution::{
    execute_transaction_flows, get_execution_result, handle_invoke_function_post_exec,
};
use crate::function_calls::create_function_calls_map;
use crate::state::ForkStateReader;
use crate::storage_changes::fetch_before_storage_changes;
use crate::transaction_extraction::extract_block_timestamp;
use crate::transaction_extraction::extract_block_txs_info;
use crate::transaction_extraction::extract_chain_id_from_felt;
use crate::transaction_extraction::extract_execution_status_transaction_receipt;
use crate::transaction_extraction::extract_starkgate_event_transaction_receipt;
use crate::transaction_extraction::extract_submitted_tx;
use crate::transaction_extraction::extract_transaction_contex;
use crate::utils::{calldata_to_hex, transaction_type_to_string};
use crate::EStarknetEvent;
use crate::FunctionCallsMap;
use crate::SimulationArgs;
use crate::SimulationInfo;
use crate::TransactionSimulationError;
use crate::TransactionSimulationResult;
use blockifier::fee::fee_checks::PostExecutionReport;
use blockifier::state::cached_state::CachedState;
use blockifier::state::errors::StateError;
use blockifier::transaction::transaction_types::TransactionType;
use cheatnet::state::CheatnetState;
use ethers::abi::AbiDecode;
use ethers::abi::AbiEncode;
use ethers::providers::Middleware;
use ethers::types::Address;
use ethers::types::Log;
use ethers::types::H256;
use ethers::types::U256;
use ethers::utils::keccak256;
use internal_tracing::build_debugger_data::debugger_data_maps_full_class_to_class;
use internal_tracing::debugger_data_fetcher::fetch_classes_debugger_data;
use internal_tracing::event_calls_map::EventCallsMap;
use internal_tracing::SimulationDebuggerData;
use num_traits::ToPrimitive;
use sqlx::Pool;
use sqlx::Postgres;
use starknet::core::types::{
    BlockId, BlockTag, ContractClass, ExecutionResult, Felt, ReceiptBlock,
};
use starknet::providers::Provider;
use starknet_api::block::BlockInfo;
use starknet_api::block::BlockNumber;
use starknet_api::block::BlockTimestamp;
use starknet_api::core::ChainId;
use starknet_api::transaction::L1HandlerTransaction;
use starknet_api::transaction::{TransactionHash, TransactionHasher, TransactionVersion};
use std::collections::HashMap;
use std::convert::TryFrom;
use tracing::warn;
use url::Url;
use walnut_shared::abi::{Enum, Event, Struct};
use walnut_shared::abi_processor::AbiProcessor;
use walnut_shared::fetch_tx_block_number_from_voyager;
use walnut_shared::parse_transaction_hash_per_network;
use walnut_shared::{
    chain_id_to_readable_string, create_eth_provider_from_url, create_rpc_client_from_url,
    to_chain_id, ETransactionHashType,
};
use walnut_shared::{EChainId, ENetwork};

pub async fn simulate(
    db_pool: &Pool<Postgres>,
    s3_client: &aws_sdk_s3::Client,
    execution_result: Option<ExecutionResult>,
    args: SimulationArgs,
) -> Result<(SimulationInfo, BlockTimestamp, usize, usize), TransactionSimulationError> {
    let provider_client = create_rpc_client_from_url(args.rpc_url.clone());
    let chain_id = args.chain_id.clone();
    let block_number = if let Some(bn) = args.block_number {
        bn.0
    } else {
        provider_client
            .block_number()
            .await
            .map_err(TransactionSimulationError::ProviderError)?
    };

    let (block_info, transaction_index, total_txs_in_block) =
        extract_block_txs_info(&provider_client, &args, block_number).await?;

    let block_timestamp = block_info.block_timestamp;
    let mut cached_fork_state = CachedState::new(
        ForkStateReader::new(
            args.rpc_url.clone(),
            block_number,
            transaction_index,
            total_txs_in_block,
            db_pool,
            s3_client,
        )
        .map_err(|e| {
            TransactionSimulationError::StateError(StateError::StateReadError(e.to_string()))
        })?,
    );
    let strkgate_event = args.strkgate_event.clone();

    let (cheatnet_state, post_execution_report) =
        run_simulation(block_info, args, &mut cached_fork_state)?;

    let ContractCallsMapBuilder {
        mut contract_calls_map,
        mut next_call_id,
        deepest_failed_contract_call_id,
        cheatnet_state_detected_events,
        ..
    } = ContractCallsMapBuilder::new_from_cheatnet_state(cheatnet_state);

    let class_hashes = contract_calls_map.collect_all_class_hashes();

    let classes_debugger_data =
        fetch_classes_debugger_data(db_pool, s3_client, &class_hashes).await;

    let (mut function_calls_map, event_calls_map, incomplete_storage_changes) =
        create_function_calls_map(
            &mut contract_calls_map,
            &mut next_call_id,
            &classes_debugger_data,
            &cached_fork_state,
        );

    let storage_changes = fetch_before_storage_changes(
        incomplete_storage_changes,
        &contract_calls_map,
        &provider_client,
        block_number,
    )
    .await;

    filter_and_hide_unlinked_function_calls(&mut contract_calls_map, &function_calls_map);

    let mut event_abis: Vec<Event> = Vec::new();
    let mut struct_abis: Vec<Struct> = Vec::new();
    let mut enum_abis: Vec<Enum> = Vec::new();

    for call in contract_calls_map.0.values_mut() {
        if let Some(class_hash) = call.entry_point.class_hash {
            let contract_class = cached_fork_state
                .state
                .in_memory_fork_cache
                .borrow()
                .get_contract_class(class_hash)
                .ok();
            if let Some(ContractClass::Sierra(class)) = contract_class {
                let mut abi_processor = AbiProcessor::new(call.entry_point.entry_point_selector);
                abi_processor.process_abi(&class.abi);
                call.entry_point_name = abi_processor.entry_point_function_name;
                call.entry_point_interface_name = abi_processor.entry_point_interface_name;
                call.is_erc20_token = abi_processor.is_erc20_token;
                call.arguments_names = abi_processor.function_arguments_names;
                call.arguments_types = abi_processor.function_arguments_types;
                call.result_types = abi_processor.function_return_result_types;
                let (sierra_version, cairo_version) =
                    extract_sierra_and_cairo_versions(&class.sierra_program);
                call.sierra_version = sierra_version;
                call.cairo_version = cairo_version;
                call.decode_call_result(&abi_processor.struct_abis, &abi_processor.enum_abis);
                call.decode_call_arguments(&abi_processor.struct_abis, &abi_processor.enum_abis);

                event_abis.extend(abi_processor.event_abis);
                struct_abis.extend(abi_processor.struct_abis);
                enum_abis.extend(abi_processor.enum_abis);
            }
        }
    }

    let mut contract_names_fetcher = ContractNamesFetcher::new(provider_client, &chain_id);
    // The StarkNet transaction emits this `Transfer` event from the StarkGate ETH token contract.
    // This event is present inside the transaction receipt but is not found in the Foundry-emitted
    // events array.
    // To maintain consistency with blockchain explorers, we need to manually append this event
    // to the vector of all events.
    let strkgate_emitted_event = if let Some(event) = strkgate_event {
        let contract_address_felt = event.from_address;
        let contract_address_str = contract_address_felt.to_fixed_hex_string();

        let contract_name = contract_names_fetcher
            .fetch_single_contract_name(contract_address_str)
            .await;
        EmittedEvent::convert_event_to_emitted_event(&event, &contract_name)
    } else {
        None
    };

    let events = EmittedEvent::create_emitted_events_list(
        &mut contract_calls_map,
        &event_abis,
        &struct_abis,
        &enum_abis,
        &cheatnet_state_detected_events,
        strkgate_emitted_event,
    );

    contract_names_fetcher
        .set_contract_names(&mut contract_calls_map)
        .await;

    let execution_result = get_execution_result(
        &contract_calls_map.0,
        deepest_failed_contract_call_id,
        post_execution_report,
        execution_result,
    )?;

    if let ExecutionResult::Reverted { reason, .. } = &execution_result {
        if let Some(call) = contract_calls_map
            .0
            .get_mut(&deepest_failed_contract_call_id.unwrap_or(1))
        {
            call.error_message = Some(reason.clone());
            call.is_deepest_panic_result = true;
        }
    }

    let debugger_trace =
        DebuggerTraceBuilder::build(&1, &mut function_calls_map, &mut contract_calls_map);

    let simulation_info = SimulationInfo {
        contract_calls_map,
        function_calls_map,
        event_calls_map,
        events,
        execution_result,
        simulation_debugger_data: Some(SimulationDebuggerData {
            classes_debugger_data: debugger_data_maps_full_class_to_class(classes_debugger_data),
            debugger_trace,
        }),
        storage_changes,
    };

    Ok((
        simulation_info,
        block_timestamp,
        transaction_index,
        total_txs_in_block,
    ))
}

fn filter_and_hide_unlinked_function_calls(
    contract_calls_map: &mut ContractCallsMap,
    function_calls_map: &FunctionCallsMap,
) {
    for contract_call in contract_calls_map.0.values_mut().filter(|c| !c.is_hidden) {
        for &child_id in &contract_call.children_call_ids {
            if contract_call.function_call_id.is_some()
                && !function_calls_map
                    .0
                    .values()
                    .any(|fc| fc.children_call_ids.contains(&child_id))
            {
                warn!("Hide function calls of the contract {:?} that has the contract call to the one that was not added by decoding system calls.",
                    contract_call.entry_point.storage_address);
                contract_call.function_call_id = None;
            }
        }
    }
}

fn run_simulation(
    block_info: BlockInfo,
    args: SimulationArgs,
    cached_fork_state: &mut CachedState<ForkStateReader>,
) -> Result<(CheatnetState, Option<PostExecutionReport>), TransactionSimulationError> {
    let signature_len = args.transaction_signature.as_ref().map_or(0, |s| s.0.len());
    let calldata_len = args.calldata.0.len();
    let transaction_context = extract_transaction_contex(
        &args.sender_address,
        &args.transaction_version,
        args.transaction_signature.clone(),
        &args.transaction_hash,
        &args.transaction_type,
        &args.nonce,
        args.chain_id.clone(),
        &block_info,
        args.max_fee,
        args.resource_bounds.clone(),
        args.paymaster_data.clone(),
    );

    let mut cheatnet_state = CheatnetState {
        block_info,
        ..Default::default()
    };

    cheatnet_state.trace_data.is_vm_trace_needed = true;

    //Execute and validation and calls
    let (validate_call_info, execute_call_info) = execute_transaction_flows(
        &args,
        cached_fork_state,
        &mut cheatnet_state,
        transaction_context.clone(),
    )?;

    //Handle post-execution for InvokeFunction
    let post_execution_report = if args.transaction_type == Some(TransactionType::InvokeFunction) {
        Some(handle_invoke_function_post_exec(
            &args,
            cached_fork_state,
            &transaction_context,
            validate_call_info,
            execute_call_info,
            signature_len,
            calldata_len,
        )?)
    } else {
        None
    };

    Ok((cheatnet_state, post_execution_report))
}

pub async fn simulate_by_calldata(
    db_pool: &Pool<Postgres>,
    s3_client: &aws_sdk_s3::Client,
    args: SimulationArgs,
) -> Result<TransactionSimulationResult, TransactionSimulationError> {
    let nonce: Option<u64> = match args.nonce {
        Some(nonce) => nonce.0.to_u64(),
        None => None,
    };
    let readable_chain_id = chain_id_to_readable_string(&args.chain_id);
    let block_number = if let Some(bn) = args.block_number {
        BlockId::Number(bn.0)
    } else {
        BlockId::Tag(BlockTag::Latest)
    };

    let sender_address = args.sender_address.0.to_string();
    let calldata = args
        .calldata
        .0
        .to_vec()
        .iter()
        .map(|felt| felt.to_hex_string())
        .collect::<Vec<String>>();
    let transaction_version: usize = args.transaction_version.0.to_u64().unwrap() as usize;
    let (
        simulation_result,
        block_timestamp,
        transaction_index_in_block,
        total_transactions_in_block,
    ) = simulate(db_pool, s3_client, None, args).await?;

    Ok(TransactionSimulationResult {
        simulation_result,
        chain_id: readable_chain_id,
        block_number,
        block_timestamp: block_timestamp.0,
        nonce,
        sender_address,
        calldata,
        transaction_version,
        transaction_type: transaction_type_to_string(TransactionType::InvokeFunction),
        transaction_index_in_block: Some(transaction_index_in_block),
        total_transactions_in_block: Some(total_transactions_in_block),
        l1_tx_hash: None,
        l2_tx_hash: None,
    })
}

pub async fn simulate_transaction_by_hash(
    db_pool: &Pool<Postgres>,
    s3_client: &aws_sdk_s3::Client,
    starknet_rpc_url: Option<Url>,
    ethereum_rpc_url: Option<String>,
    tx_hash: &str,
    chain_id: Option<EChainId>,
    network: &ENetwork,
) -> Result<TransactionSimulationResult, TransactionSimulationError> {
    if let Some(transaction_hash) = parse_transaction_hash_per_network(tx_hash, network) {
        let result = match transaction_hash {
            ETransactionHashType::Starknet(starknet_hash) => {
                simulate_starknet_transaction_by_hash(
                    db_pool,
                    s3_client,
                    starknet_rpc_url,
                    ethereum_rpc_url,
                    starknet_hash,
                    chain_id,
                )
                .await?
            }
            ETransactionHashType::Ethereum(ethereum_hash) => {
                simulate_ethereum_transaction_by_hash(
                    db_pool,
                    s3_client,
                    starknet_rpc_url,
                    ethereum_rpc_url,
                    ethereum_hash,
                    chain_id,
                )
                .await?
            }
        };
        Ok(result)
    } else {
        Err(TransactionSimulationError::TransactionHashNotFound)
    }
}

async fn simulate_starknet_transaction_by_hash(
    db_pool: &Pool<Postgres>,
    s3_client: &aws_sdk_s3::Client,
    starknet_rpc_url: Option<Url>,
    _ethereum_rpc_url: Option<String>,
    transaction_hash: Felt,
    chain_id: Option<EChainId>,
) -> Result<TransactionSimulationResult, TransactionSimulationError> {
    let starknet_rpc_url = starknet_rpc_url.ok_or(TransactionSimulationError::InvalidRpcUrl)?;
    let provider_client = create_rpc_client_from_url(starknet_rpc_url.clone());
    // Fetch transaction
    let transaction = provider_client
        .get_transaction_by_hash(transaction_hash)
        .await;
    if let Ok(transaction) = transaction {
        if let Some((
            nonce,
            sender_address,
            entry_point_selector,
            calldata,
            transaction_version,
            transaction_type,
            signature,
            max_fee,
            resource_bounds,
            paymaster_data,
        )) = extract_submitted_tx(transaction)
        {
            // Fetch receipt
            let transaction_receipt_with_block_info = provider_client
                .get_transaction_receipt(transaction_hash)
                .await;
            if let Ok(transaction_receipt_with_block_info) = transaction_receipt_with_block_info {
                let transaction_receipt = transaction_receipt_with_block_info.receipt;
                let block_info = transaction_receipt_with_block_info.block;

                match block_info {
                    ReceiptBlock::Pending => {
                        return Err(TransactionSimulationError::PendingBlock(
                            "Transaction simualtion in pending block not found".to_string(),
                        ));
                    }
                    ReceiptBlock::Block { block_number, .. } => {
                        // Fetch or derive chain_id
                        let chain_id = match chain_id {
                            Some(chain_id) => ChainId::from(chain_id),
                            None => extract_chain_id_from_felt(
                                provider_client.chain_id().await.map_err(|_| {
                                    TransactionSimulationError::FailedToFetchChainId
                                })?,
                            )?,
                        };

                        let tx_execution_result =
                            extract_execution_status_transaction_receipt(&transaction_receipt);
                        // Check for execution status
                        if let Some(ExecutionResult::Reverted { ref reason }) = tx_execution_result
                        {
                            if reason.contains("RunResources") {
                                // Fetch block timestamp
                                let block_timestamp =
                                    extract_block_timestamp(&provider_client, block_number).await?;

                                let simulation_info = SimulationInfo {
                                    contract_calls_map: ContractCallsMap::new(),
                                    function_calls_map: FunctionCallsMap::new(),
                                    event_calls_map: EventCallsMap::default(),
                                    events: Vec::new(),
                                    execution_result: ExecutionResult::Reverted {
                                        reason: reason.to_string(),
                                    },
                                    simulation_debugger_data: Some(SimulationDebuggerData {
                                        classes_debugger_data: HashMap::new(),
                                        debugger_trace: Vec::new(),
                                    }),
                                    storage_changes: HashMap::new(),
                                };
                                return Ok(TransactionSimulationResult {
                                    simulation_result: simulation_info,
                                    chain_id: chain_id_to_readable_string(&chain_id),
                                    block_number: BlockId::Number(block_number),
                                    block_timestamp: block_timestamp.0,
                                    nonce: nonce.0.to_u64(),
                                    sender_address: sender_address.0.to_string(),
                                    calldata: calldata_to_hex(&calldata),
                                    transaction_version: transaction_version.0.to_u64().unwrap()
                                        as usize,
                                    transaction_type: transaction_type_to_string(transaction_type),
                                    transaction_index_in_block: None,
                                    total_transactions_in_block: None,
                                    l1_tx_hash: None,
                                    l2_tx_hash: Some(transaction_hash.to_hex_string()),
                                });
                            }
                        }

                        let strkgate_event =
                            extract_starkgate_event_transaction_receipt(&transaction_receipt);
                        // Perform transaction simulation
                        let (
                            simulation_result,
                            block_timestamp,
                            transaction_index_in_block,
                            total_transactions_in_block,
                        ) = simulate(
                            db_pool,
                            s3_client,
                            tx_execution_result,
                            SimulationArgs {
                                rpc_url: starknet_rpc_url.clone(),
                                chain_id: chain_id.clone(),
                                block_number: Some(BlockNumber(block_number)),
                                nonce: Some(nonce),
                                sender_address,
                                entry_point_selector: Some(entry_point_selector),
                                calldata: calldata.clone(),
                                transaction_version,
                                transaction_signature: Some(signature),
                                transaction_hash: Some(TransactionHash(transaction_hash)),
                                transaction_type: Some(transaction_type),
                                max_fee: Some(max_fee),
                                resource_bounds: Some(resource_bounds),
                                paymaster_data: Some(paymaster_data),
                                strkgate_event,
                            },
                        )
                        .await?;
                        // Build and return simulation result
                        return Ok(TransactionSimulationResult {
                            simulation_result,
                            chain_id: chain_id_to_readable_string(&chain_id),
                            block_number: BlockId::Number(block_number),
                            block_timestamp: block_timestamp.0,
                            nonce: nonce.0.to_u64(),
                            sender_address: sender_address.0.to_string(),
                            calldata: calldata_to_hex(&calldata),
                            transaction_version: transaction_version.0.to_u64().unwrap() as usize,
                            transaction_type: transaction_type_to_string(transaction_type),
                            transaction_index_in_block: Some(transaction_index_in_block),
                            total_transactions_in_block: Some(total_transactions_in_block),
                            l1_tx_hash: None,
                            l2_tx_hash: Some(transaction_hash.to_hex_string()),
                        });
                    }
                }
            }
        }
    }
    Err(TransactionSimulationError::TransactionHashNotFound)
}

pub async fn simulate_ethereum_transaction_by_hash(
    db_pool: &Pool<Postgres>,
    s3_client: &aws_sdk_s3::Client,
    starknet_rpc_url: Option<Url>,
    ethereum_rpc_url: Option<String>,
    transaction_hash: H256,
    chain_id: Option<EChainId>,
) -> Result<TransactionSimulationResult, TransactionSimulationError> {
    let chain_id = chain_id.ok_or(TransactionSimulationError::InvalidChainId)?;
    let ethereum_rpc_url = ethereum_rpc_url.ok_or(TransactionSimulationError::InvalidRpcUrl)?;

    // Fetch the tx from L1HandlerTransaction
    let provider_eth_client = create_eth_provider_from_url(ethereum_rpc_url);
    let transaction_receipt = provider_eth_client
        .get_transaction_receipt(transaction_hash)
        .await;

    match transaction_receipt {
        Ok(Some(receipt)) => {
            for log in receipt.logs {
                if let Some(l1_event) = decode_l1_event(&log) {
                    // Convert the event data to L1HandlerTransaction
                    let l1_handler_tx = L1HandlerTransaction::try_from(l1_event)
                        .expect("Can not convert L1 event to L1 Handler tx type");
                    let l2_chain_id = to_chain_id(&chain_id);
                    let core_l2_chain_id = ChainId::from(l2_chain_id);
                    let l2_tx_hash: Option<TransactionHash> = l1_handler_tx
                        .calculate_transaction_hash(
                            &core_l2_chain_id,
                            &starknet_api::transaction::TransactionVersion::ZERO,
                        )
                        .ok();
                    let l2_tx_hash_hex = l2_tx_hash.map(|hash| hash.to_hex_string());
                    let block_number = match &l2_tx_hash_hex {
                        Some(l2_tx_hash_hex) => {
                            fetch_tx_block_number_from_voyager(&core_l2_chain_id, l2_tx_hash_hex)
                                .await
                                .ok()
                        }
                        None => None,
                    };

                    let starknet_rpc_url =
                        starknet_rpc_url.ok_or(TransactionSimulationError::InvalidRpcUrl)?;

                    let (
                        simulation_result,
                        block_timestamp,
                        transaction_index_in_block,
                        total_transactions_in_block,
                    ) = simulate(
                        db_pool,
                        s3_client,
                        None,
                        SimulationArgs {
                            rpc_url: starknet_rpc_url.clone(),
                            chain_id: core_l2_chain_id.clone(),
                            block_number: block_number.map(|bn| BlockNumber(bn)),
                            nonce: Some(l1_handler_tx.nonce),
                            sender_address: l1_handler_tx.contract_address,
                            entry_point_selector: Some(l1_handler_tx.entry_point_selector),
                            calldata: l1_handler_tx.calldata.clone(),
                            transaction_version: TransactionVersion::ZERO,
                            transaction_signature: None,
                            transaction_hash: l2_tx_hash,
                            transaction_type: Some(TransactionType::L1Handler),
                            max_fee: None,
                            resource_bounds: None,
                            paymaster_data: None,
                            strkgate_event: None,
                        },
                    )
                    .await?;
                    return Ok(TransactionSimulationResult {
                        simulation_result,
                        chain_id: chain_id_to_readable_string(&core_l2_chain_id),
                        block_number: block_number.map(|bn| BlockId::Number(bn)).unwrap_or(
                            starknet::core::types::BlockId::Tag(
                                starknet::core::types::BlockTag::Latest,
                            ),
                        ),
                        block_timestamp: block_timestamp.0,
                        nonce: l1_handler_tx.nonce.0.to_u64(),
                        sender_address: l1_handler_tx.contract_address.to_string(),
                        calldata: calldata_to_hex(&l1_handler_tx.calldata),
                        transaction_version: TransactionVersion::ZERO.0.to_u64().unwrap() as usize,
                        transaction_type: transaction_type_to_string(TransactionType::L1Handler),
                        transaction_index_in_block: Some(transaction_index_in_block),
                        total_transactions_in_block: Some(total_transactions_in_block),
                        l1_tx_hash: Some(transaction_hash.encode_hex()),
                        l2_tx_hash: l2_tx_hash_hex,
                    });
                }
            }
            Err(TransactionSimulationError::TransactionHashNotFound)
        }
        Ok(None) => Err(TransactionSimulationError::TransactionHashNotFound),
        Err(_) => Err(TransactionSimulationError::TransactionHashNotFound),
    }
}

fn decode_l1_event(log: &Log) -> Option<EStarknetEvent> {
    let topics = log.topics.clone();
    if topics.is_empty() {
        return None;
    }

    let event_signature = topics[0];
    // TODO: Add other events
    match event_signature {
        sig if sig == H256::from(keccak256("LogMessageToL1(uint256,address,uint256[])")) => {
            if topics.len() < 3 {
                return None;
            }

            let from_address = format!("{:?}", Address::from(topics[1]));
            let to_address = topics[2].to_string();

            <Vec<U256>>::decode(&log.data)
                .ok()
                .map(|decoded| EStarknetEvent::LogMessageToL1 {
                    from_address,
                    to_address,
                    payload: decoded.iter().map(U256::to_string).collect(),
                })
        }

        sig if sig
            == H256::from(keccak256(
                "LogMessageToL2(address,uint256,uint256,uint256[],uint256,uint256)",
            )) =>
        {
            if topics.len() < 4 {
                return None;
            }

            let from_address = Address::from(topics[1]);
            let to_address = U256::from_big_endian(topics[2].as_fixed_bytes());
            let selector = U256::from_big_endian(topics[3].as_fixed_bytes());

            <(Vec<U256>, U256, U256)>::decode(&log.data)
                .ok()
                .map(|(payload, nonce, fee)| EStarknetEvent::LogMessageToL2 {
                    from_address,
                    to_address,
                    selector,
                    payload,
                    nonce,
                    fee,
                })
        }

        _ => None,
    }
}

fn extract_sierra_and_cairo_versions(sierra_program: &[Felt]) -> (Option<String>, Option<String>) {
    if sierra_program.len() < 6 {
        return (None, None);
    }
    let sierra_version = format!(
        "{}.{}.{}",
        sierra_program[0], sierra_program[1], sierra_program[2]
    );

    let cairo_version = format!(
        "{}.{}.{}",
        sierra_program[3], sierra_program[4], sierra_program[5]
    );

    (Some(sierra_version), Some(cairo_version))
}
