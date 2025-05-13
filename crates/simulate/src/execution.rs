use crate::state::ForkStateReader;
use crate::ContractCall;
use crate::DetailedTransactionReceipt;
use crate::SimulationArgs;
use crate::TransactionSimulationError;
use blockifier::context::TransactionContext;
use blockifier::execution::call_info::CallInfo;
use blockifier::execution::call_info::ExecutionSummary;
use blockifier::execution::common_hints::ExecutionMode;
use blockifier::execution::contract_class::RunnableCompiledClass as BlockifierContractClass;
use blockifier::execution::entry_point::CallEntryPoint;
use blockifier::execution::entry_point::CallType;
use blockifier::execution::entry_point::EntryPointExecutionContext;
use blockifier::execution::entry_point::SierraGasRevertTracker;
use blockifier::fee::fee_checks::PostExecutionReport;
use blockifier::fee::fee_utils::get_vm_resources_cost;
use blockifier::fee::receipt::TransactionReceipt;
use blockifier::fee::resources::ComputationResources;
use blockifier::fee::resources::StarknetResources;
use blockifier::fee::resources::StateResources;
use blockifier::fee::resources::TransactionResources;
use blockifier::state::cached_state::CachedState;
use blockifier::state::cached_state::StateChanges;
use blockifier::state::state_api::State;
use blockifier::transaction::errors::TransactionExecutionError;
use blockifier::transaction::objects::HasRelatedFeeType;
use blockifier::transaction::transaction_types::TransactionType;
use cheatnet::runtime_extensions::call_to_blockifier_runtime_extension::execution::entry_point::execute_call_entry_point;
use cheatnet::runtime_extensions::call_to_blockifier_runtime_extension::rpc::CallFailure;
use cheatnet::runtime_extensions::call_to_blockifier_runtime_extension::rpc::CallResult;
use cheatnet::state::CheatnetState;
use starknet::core::types::ExecutionResult;
use starknet::core::types::Felt;
use starknet_api::abi::abi_utils::selector_from_name;
use starknet_api::calldata;
use starknet_api::contract_class::EntryPointType;
use starknet_api::core::ContractAddress;
use starknet_api::core::EntryPointSelector;
use starknet_api::execution_resources::GasAmount;
use starknet_api::execution_resources::GasVector;
use starknet_api::transaction::constants;
use starknet_api::transaction::fields::Calldata;
use starknet_api::transaction::fields::Fee;
use starknet_api::transaction::fields::GasVectorComputationMode;
use std::collections::HashMap;
use std::sync::Arc;
use walnut_shared::felts_to_string;

pub fn execute_transaction_flows(
    args: &SimulationArgs,
    cached_fork_state: &mut CachedState<ForkStateReader>,
    cheatnet_state: &mut CheatnetState,
    transaction_context: Arc<TransactionContext>,
) -> Result<(Option<CallInfo>, Option<CallInfo>), TransactionSimulationError> {
    let validate_call_info = if should_validate(args) {
        let selector = get_validate_selector(args.transaction_type);
        validate_call(
            args.calldata.clone(),
            args.sender_address,
            selector,
            cached_fork_state,
            cheatnet_state,
            transaction_context.clone(),
            u64::MAX,
        )
        .ok()
    } else {
        None
    };

    // Ensure we have validation info if required
    if should_validate(args) && validate_call_info.is_none() {
        return Err(TransactionSimulationError::OtherError(
            "Transaction validate error".to_string(),
        ));
    }

    let mut execution_context = EntryPointExecutionContext::new(
        transaction_context.clone(),
        ExecutionMode::Execute,
        false,
        SierraGasRevertTracker::new(GasAmount(u64::MAX)),
    );

    if args.transaction_type.is_none() {
        return Err(TransactionSimulationError::OtherError(
            "Transaction type must be known".to_string(),
        ));
    }

    let execute_call_info = if should_execute(args) {
        execute_call(
            args.entry_point_selector,
            args.calldata.clone(),
            args.sender_address,
            args.transaction_type,
            cached_fork_state,
            cheatnet_state,
            u64::MAX,
            &mut execution_context,
        )
        .ok()
    } else {
        None
    };

    Ok((validate_call_info, execute_call_info))
}

pub fn handle_post_exec_and_collect_gas_vectors(
    args: &SimulationArgs,
    cached_fork_state: &mut CachedState<ForkStateReader>,
    cheatnet_state: &mut CheatnetState,
    transaction_context: Arc<TransactionContext>,
    validate_call_info: Option<CallInfo>,
    execute_call_info: Option<CallInfo>,
    signature_len: usize,
    calldata_len: usize,
) -> Result<(PostExecutionReport, DetailedTransactionReceipt), TransactionSimulationError> {
    let state_changes = cached_fork_state.get_actual_state_changes()?;

    let versioned_constants = transaction_context.block_context.versioned_constants();
    let use_kzg_da = transaction_context.block_context.block_info().use_kzg_da;
    let gas_mode = transaction_context.get_gas_vector_computation_mode();

    let execution_summary = CallInfo::summarize_many(
        validate_call_info.iter().chain(execute_call_info.iter()),
        versioned_constants,
    );
    let tx_resources = calculate_transaction_resources(
        args,
        &transaction_context,
        &execution_summary,
        signature_len,
        calldata_len,
        &state_changes,
    );

    let tx_receipt = create_transaction_receipt(&transaction_context, &tx_resources);

    let post_exec_report =
        PostExecutionReport::new(cached_fork_state, &transaction_context, &tx_receipt, true)?;

    let _fee_call_info = if args.transaction_type == Some(TransactionType::InvokeFunction) {
        let fee = tx_receipt.fee;
        Some(execute_fee_transfer(
            cached_fork_state,
            cheatnet_state,
            transaction_context.clone(),
            fee,
            versioned_constants
                .os_constants
                .gas_costs
                .base
                .default_initial_gas_cost,
        )?)
    } else {
        None
    };

    let fee_state_changes = cached_fork_state.get_actual_state_changes()?;

    let tx_resources = calculate_transaction_resources(
        args,
        &transaction_context,
        &execution_summary,
        signature_len,
        calldata_len,
        &fee_state_changes,
    );

    let tx_receipt = create_transaction_receipt(&transaction_context, &tx_resources);

    let starknet_resources = tx_receipt.resources.starknet_resources;
    let computation_resources = tx_receipt.resources.computation;

    let starknet_resources_archival_data_gas_vector = starknet_resources
        .archival_data
        .to_gas_vector(versioned_constants, &gas_mode);
    let starknet_resources_message_gas_vector = starknet_resources.messages.to_gas_vector();
    let starknet_resources_state_gas_vector = starknet_resources
        .state
        .to_gas_vector(use_kzg_da, &versioned_constants.allocation_cost);
    let starknet_resources_gas_vector =
        starknet_resources.to_gas_vector(versioned_constants, use_kzg_da, &gas_mode);

    let computation_resources_vm_cost_gas_vector = get_vm_resources_cost(
        versioned_constants,
        &computation_resources.vm_resources,
        computation_resources.n_reverted_steps,
        &gas_mode,
    );
    let total_sierra_gas = computation_resources
        .sierra_gas
        .checked_add(computation_resources.reverted_sierra_gas)
        .unwrap_or_default();
    let computation_resources_sierra_gas_vector = match gas_mode {
        GasVectorComputationMode::All => GasVector::from_l2_gas(total_sierra_gas),
        GasVectorComputationMode::NoL2Gas => GasVector::from_l1_gas(
            versioned_constants.sierra_gas_to_l1_gas_amount_round_up(total_sierra_gas),
        ),
    };
    let computation_resources_gas_vector =
        computation_resources.to_gas_vector(versioned_constants, &gas_mode);

    let detailed_receipt = DetailedTransactionReceipt {
        fee: tx_receipt.fee,
        gas: tx_receipt.gas,
        da_gas: tx_receipt.da_gas,

        starknet_resources_gas_vector,
        starknet_resources_archival_data_gas_vector,
        starknet_resources_message_gas_vector,
        starknet_resources_state_gas_vector,

        computation_resources_vm_cost_gas_vector,
        computation_resources_sierra_gas_vector,
        computation_resources_gas_vector,
    };

    Ok((post_exec_report, detailed_receipt))
}

pub fn get_execution_result(
    contract_calls_map: &HashMap<u32, ContractCall>,
    deepest_contract_call_id: Option<u32>,
    post_execution_report: Option<PostExecutionReport>,
    execution_result: Option<ExecutionResult>,
) -> Result<ExecutionResult, TransactionSimulationError> {
    if let Some(_post_execution_report) = post_execution_report {
        if let Some(ExecutionResult::Reverted { reason }) = execution_result {
            return Ok(ExecutionResult::Reverted { reason });
        }
    }
    if let Some(deepest_contract_call_id) = deepest_contract_call_id {
        if let Some(call) = contract_calls_map.get(&deepest_contract_call_id) {
            if let CallResult::Failure(failure) = &call.result {
                match failure {
                    CallFailure::Panic { panic_data } => {
                        let reason = felts_to_string(panic_data);
                        Ok(ExecutionResult::Reverted {
                            reason: reason.trim().to_string(),
                        })
                    }
                    CallFailure::Error { msg } => {
                        let reason = msg.to_string().trim().to_string();
                        Ok(ExecutionResult::Reverted { reason })
                    }
                }
            } else {
                Ok(ExecutionResult::Succeeded)
            }
        } else {
            unreachable!("deepest_contract_call_id not found in contract_calls_map");
        }
    } else {
        Ok(ExecutionResult::Succeeded)
    }
}

fn calculate_transaction_resources(
    args: &SimulationArgs,
    transaction_context: &TransactionContext,
    execution_summary: &ExecutionSummary,
    signature_len: usize,
    calldata_len: usize,
    state_changes: &StateChanges,
) -> TransactionResources {
    let charged_resources = execution_summary.charged_resources.clone();
    let versioned_constants = transaction_context.block_context.versioned_constants();
    let use_kzg_da = transaction_context.block_context.block_info().use_kzg_da;
    let gas_mode = transaction_context.get_gas_vector_computation_mode();

    let state_changes_count = state_changes.count_for_fee_charge(
        Some(args.sender_address),
        transaction_context
            .block_context
            .chain_info()
            .fee_token_address(&transaction_context.tx_info.fee_type()),
    );

    let state_resources = StateResources {
        state_changes_for_fee: state_changes_count,
    };

    let starknet_resources = StarknetResources::new(
        calldata_len,
        signature_len,
        0,
        state_resources,
        None,
        execution_summary.clone(),
    );

    let addition_os_resources = versioned_constants
        .get_additional_os_tx_resources(
            TransactionType::InvokeFunction,
            &starknet_resources,
            use_kzg_da,
        )
        .filter_unused_builtins();
    let total_vm_resources = &charged_resources.vm_resources + &addition_os_resources;

    let computation_resources = ComputationResources {
        vm_resources: total_vm_resources,
        n_reverted_steps: 0,
        sierra_gas: charged_resources.gas_consumed,
        reverted_sierra_gas: GasAmount(0),
    };

    TransactionResources {
        starknet_resources,
        computation: computation_resources,
    }
}

fn create_transaction_receipt(
    transaction_context: &TransactionContext,
    resources: &TransactionResources,
) -> TransactionReceipt {
    let versioned_constants = transaction_context.block_context.versioned_constants();
    let gas_mode = transaction_context.get_gas_vector_computation_mode();

    let gas = resources.to_gas_vector(
        versioned_constants,
        transaction_context.block_context.block_info().use_kzg_da,
        &gas_mode,
    );

    let fee = transaction_context
        .tx_info
        .get_fee_by_gas_vector(transaction_context.block_context.block_info(), gas);

    let da_gas = resources
        .starknet_resources
        .state
        .da_gas_vector(transaction_context.block_context.block_info().use_kzg_da);

    TransactionReceipt {
        fee,
        gas,
        da_gas,
        resources: resources.clone(),
    }
}

fn should_validate(args: &SimulationArgs) -> bool {
    args.transaction_hash.is_some() && args.transaction_type != Some(TransactionType::L1Handler)
}

fn should_execute(args: &SimulationArgs) -> bool {
    args.transaction_type.is_none() || args.transaction_type != Some(TransactionType::Declare)
}

fn get_validate_selector(tx_type: Option<TransactionType>) -> EntryPointSelector {
    match tx_type {
        Some(TransactionType::Declare) => {
            selector_from_name(constants::VALIDATE_DECLARE_ENTRY_POINT_NAME)
        }
        _ => selector_from_name(constants::VALIDATE_ENTRY_POINT_NAME),
    }
}

fn validate_call(
    calldata: Calldata,
    storage_address: ContractAddress,
    validate_selector: EntryPointSelector,
    state: &mut dyn State,
    cheatnet_state: &mut CheatnetState,
    tx_context: Arc<TransactionContext>,
    initial_gas: u64,
) -> Result<CallInfo, TransactionSimulationError> {
    let mut validation_context = EntryPointExecutionContext::new(
        tx_context.clone(),
        ExecutionMode::Validate,
        false,
        SierraGasRevertTracker::new(GasAmount(initial_gas)),
    );

    let class_hash = state.get_class_hash_at(storage_address)?;

    let mut validate_call = CallEntryPoint {
        entry_point_type: EntryPointType::External,
        entry_point_selector: validate_selector,
        calldata,
        class_hash: None,
        code_address: None,
        storage_address,
        caller_address: ContractAddress::default(),
        call_type: CallType::Call,
        initial_gas,
    };

    let validate_call_info = execute_call_entry_point(
        &mut validate_call,
        state,
        cheatnet_state,
        &mut validation_context,
    );

    let validate_call_info = match validate_call_info {
        Ok(info) => info,
        Err(err) => {
            return Err(TransactionSimulationError::TransactionExecutionError(
                TransactionExecutionError::ExecutionError {
                    error: err,
                    class_hash,
                    storage_address,
                    selector: validate_selector,
                },
            ));
        }
    };
    let contract_class = state.get_compiled_class(class_hash)?;
    if matches!(
        contract_class,
        BlockifierContractClass::V0(_) | BlockifierContractClass::V1(_)
    ) {
        let expected_retdata = vec![*constants::VALIDATE_RETDATA];

        if validate_call_info.execution.retdata.0 != expected_retdata {
            return Err(TransactionSimulationError::TransactionExecutionError(
                TransactionExecutionError::InvalidValidateReturnData {
                    actual: validate_call_info.execution.retdata,
                },
            ));
        }
    }

    Ok(validate_call_info)
}

fn execute_call(
    entry_point_selector: Option<EntryPointSelector>,
    calldata: Calldata,
    storage_address: ContractAddress,
    transaction_type: Option<TransactionType>,
    state: &mut dyn State,
    cheatnet_state: &mut CheatnetState,
    initial_gas: u64,
    execution_context: &mut EntryPointExecutionContext,
) -> Result<CallInfo, TransactionSimulationError> {
    let execute_entry_point_selector = selector_from_name(constants::EXECUTE_ENTRY_POINT_NAME);

    let mut execute_call = CallEntryPoint {
        entry_point_type: EntryPointType::External,
        entry_point_selector: execute_entry_point_selector,
        calldata: calldata.clone(),
        class_hash: None,
        code_address: None,
        storage_address,
        caller_address: ContractAddress::default(),
        call_type: CallType::Call,
        initial_gas,
    };

    if transaction_type.is_some()
        && transaction_type == Some(TransactionType::L1Handler)
        && entry_point_selector.is_some()
    {
        execute_call = CallEntryPoint {
            entry_point_type: EntryPointType::L1Handler,
            entry_point_selector: entry_point_selector.unwrap(),
            calldata,
            class_hash: None,
            code_address: None,
            storage_address,
            caller_address: ContractAddress::default(),
            call_type: CallType::Call,
            initial_gas,
        }
    }

    let execution_result =
        execute_call_entry_point(&mut execute_call, state, cheatnet_state, execution_context)?;

    Ok(execution_result)
}

pub fn execute_fee_transfer(
    state: &mut dyn State,
    cheatnet_state: &mut CheatnetState,
    tx_context: Arc<TransactionContext>,
    actual_fee: Fee,
    initial_gas: u64,
) -> Result<CallInfo, TransactionSimulationError> {
    let mut execution_context = EntryPointExecutionContext::new(
        tx_context.clone(),
        ExecutionMode::Execute,
        false,
        SierraGasRevertTracker::new(GasAmount(initial_gas)),
    );

    let storage_address = tx_context.fee_token_address();
    let lsb_amount = Felt::from(actual_fee.0);
    let msb_amount = Felt::ZERO;

    let mut fee_transfer_call = CallEntryPoint {
        entry_point_type: EntryPointType::External,
        entry_point_selector: selector_from_name(constants::TRANSFER_ENTRY_POINT_NAME),
        calldata: calldata![
            *tx_context
                .block_context
                .block_info()
                .sequencer_address
                .0
                .key(),
            lsb_amount,
            msb_amount
        ],
        class_hash: None,
        code_address: None,
        storage_address,
        caller_address: tx_context.tx_info.sender_address(),
        call_type: CallType::Call,
        initial_gas,
    };

    let execution_result = execute_call_entry_point(
        &mut fee_transfer_call,
        state,
        cheatnet_state,
        &mut execution_context,
    )?;

    Ok(execution_result)
}
