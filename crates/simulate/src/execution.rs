// These execution helpers share the same wide set of inputs.
#![allow(clippy::too_many_arguments)]

use crate::convert_entry_point_error_with_block;
use crate::gas_counter::GasCounter;
use crate::state::ForkStateReader;
use crate::utils::format_fee_string;
use crate::ContractCall;
use crate::DetailedTransactionReceipt;
use crate::SimulationArgs;
use crate::TransactionSimulationError;
use blockifier::blockifier_versioned_constants::VersionedConstants;
use blockifier::context::TransactionContext;
use blockifier::execution::call_info::CallInfo;
use blockifier::execution::call_info::ExecutionSummary;
use blockifier::execution::common_hints::ExecutionMode;
use blockifier::execution::entry_point::CallEntryPoint;
use blockifier::execution::entry_point::CallType;
use blockifier::execution::entry_point::EntryPointExecutionContext;
use blockifier::execution::entry_point::SierraGasRevertTracker;
use blockifier::execution::errors::EntryPointExecutionError;
use blockifier::fee::fee_checks::PostExecutionReport;
use blockifier::fee::fee_utils::get_extended_vm_resources_cost;
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
use cheatnet::runtime_extensions::call_to_blockifier_runtime_extension::execution::entry_point::{
    execute_call_entry_point, ExecuteCallEntryPointExtraOptions,
};
use cheatnet::runtime_extensions::call_to_blockifier_runtime_extension::rpc::CallFailure;
use cheatnet::state::CheatnetState;
use starknet_api::abi::abi_utils::selector_from_name;
use starknet_api::calldata;
use starknet_api::contract_class::EntryPointType;
use starknet_api::core::ContractAddress;
use starknet_api::core::EntryPointSelector;
use starknet_api::executable_transaction::TransactionType;
use starknet_api::execution_resources::GasAmount;
use starknet_api::execution_resources::GasVector;
use starknet_api::transaction::constants;
use starknet_api::transaction::fields::Calldata;
use starknet_api::transaction::fields::Fee;
use starknet_api::transaction::fields::GasVectorComputationMode;
use starknet_api::transaction::TransactionVersion;
use starknet_rust::core::types::ExecutionResult;
use starknet_rust::core::types::Felt;
use std::collections::HashMap;
use std::sync::Arc;
use tracing::instrument;
use walnut_shared::felts_to_string;

type CallEntryPointExecutor<'a> = dyn Fn(
        &mut CallEntryPoint,
        &mut dyn State,
        &mut CheatnetState,
        &mut EntryPointExecutionContext,
        bool,
    ) -> Result<CallInfo, TransactionSimulationError>
    + 'a;

pub struct PostExecStateData {
    pub state_changes: StateChanges,
    pub starknet_resources_state: StateResources,
    pub post_exec_report: PostExecutionReport,
    pub detailed_receipt: DetailedTransactionReceipt,
}

#[instrument(name = "validate_call", skip_all, fields(storage_address = %storage_address.0.key().to_fixed_hex_string(), is_revertable))]
fn validate_call_with_executor(
    calldata: Calldata,
    storage_address: ContractAddress,
    validate_selector: EntryPointSelector,
    state: &mut dyn State,
    cheatnet_state: &mut CheatnetState,
    tx_context: Arc<TransactionContext>,
    remaining_gas: &mut GasCounter,
    is_revertable: bool,
    executor: &CallEntryPointExecutor,
) -> Result<CallInfo, TransactionSimulationError> {
    let remaining_validation_gas = &mut remaining_gas.limit_usage(
        tx_context
            .block_context
            .versioned_constants()
            .os_constants
            .validate_max_sierra_gas,
    );
    let mut validation_context = EntryPointExecutionContext::new(
        tx_context.clone(),
        ExecutionMode::Validate,
        false,
        SierraGasRevertTracker::new(GasAmount(*remaining_validation_gas)),
    );

    let mut validate_call = CallEntryPoint {
        entry_point_type: EntryPointType::External,
        entry_point_selector: validate_selector,
        calldata,
        class_hash: None,
        code_address: None,
        storage_address,
        caller_address: ContractAddress::default(),
        call_type: CallType::Call,
        initial_gas: *remaining_validation_gas,
    };

    let validate_call_info = executor(
        &mut validate_call,
        state,
        cheatnet_state,
        &mut validation_context,
        is_revertable,
    )
    .map_err(|_err| {
        TransactionSimulationError::EntryPointExecutionError(
            EntryPointExecutionError::InternalError("Validate call error".to_string()),
        )
    })?;

    let expected_retdata = vec![*constants::VALIDATE_RETDATA];
    if validate_call_info.execution.retdata.0 != expected_retdata {
        return Err(TransactionSimulationError::TransactionExecutionError(
            TransactionExecutionError::InvalidValidateReturnData {
                actual: validate_call_info.execution.retdata,
            },
        ));
    }

    remaining_gas.subtract_used_gas(&validate_call_info);
    Ok(validate_call_info)
}

#[instrument(name = "execute_tx_flows", skip_all, fields(
    sender = %args.sender_address.0.key().to_fixed_hex_string(),
    tx_type = ?args.transaction_type,
    tx_version = %args.transaction_version.0,
))]
pub fn execute_transaction_flows_with_executor<'a>(
    args: &SimulationArgs,
    cached_fork_state: &mut CachedState<ForkStateReader>,
    cheatnet_state: &mut CheatnetState,
    remaining_gas: &mut GasCounter,
    transaction_context: Arc<TransactionContext>,
    validate_executor: &'a CallEntryPointExecutor,
    execute_executor: &'a CallEntryPointExecutor,
) -> Result<(Option<CallInfo>, Option<CallInfo>), TransactionSimulationError> {
    let tx_type = args.transaction_type.ok_or_else(|| {
        TransactionSimulationError::OtherError("Transaction type must be known".into())
    })?;
    let is_revertable = is_transaction_revertible(tx_type, &args.transaction_version);

    let validate_call_info = if should_validate(args) {
        let selector = get_validate_selector(args.transaction_type);
        validate_call_with_executor(
            args.calldata.clone(),
            args.sender_address,
            selector,
            cached_fork_state,
            cheatnet_state,
            transaction_context.clone(),
            remaining_gas,
            is_revertable,
            validate_executor,
        )
        .ok()
    } else {
        None
    };

    let mut execution_context = EntryPointExecutionContext::new(
        transaction_context.clone(),
        ExecutionMode::Execute,
        false,
        SierraGasRevertTracker::new(GasAmount(
            remaining_gas.limit_usage(
                transaction_context
                    .block_context
                    .versioned_constants()
                    .sierra_gas_limit(&ExecutionMode::Execute),
            ),
        )),
    );

    let execute_call_info = if should_execute(args) {
        let ep_selector = args.entry_point_selector;
        let storage_address = args.sender_address;
        let tx_type = args.transaction_type;
        let sierra_gas_limit = transaction_context
            .block_context
            .versioned_constants()
            .sierra_gas_limit(&ExecutionMode::Execute);
        let remaining_execution_gas = remaining_gas.limit_usage(sierra_gas_limit);

        let mut execute_call = CallEntryPoint {
            entry_point_type: if tx_type == Some(TransactionType::L1Handler)
                && ep_selector.is_some()
            {
                EntryPointType::L1Handler
            } else {
                EntryPointType::External
            },
            entry_point_selector: if tx_type == Some(TransactionType::L1Handler)
                && ep_selector.is_some()
            {
                ep_selector.unwrap()
            } else {
                get_entrypoint_selector()
            },
            calldata: args.calldata.clone(),
            class_hash: None,
            code_address: None,
            storage_address,
            caller_address: ContractAddress::default(),
            call_type: CallType::Call,
            initial_gas: if tx_type == Some(TransactionType::L1Handler) && ep_selector.is_some() {
                u64::MAX
            } else {
                remaining_execution_gas
            },
        };

        execute_executor(
            &mut execute_call,
            cached_fork_state,
            cheatnet_state,
            &mut execution_context,
            is_revertable,
        )
        .map_err(|err: TransactionSimulationError| {
            // If it's an EntryPointExecutionError, try to extract block number and convert
            if let TransactionSimulationError::EntryPointExecutionError(ep_err) = err {
                // Extract block number from state (the block where transaction is executed)
                let block_number = cached_fork_state.state.block_number();
                convert_entry_point_error_with_block(ep_err, block_number)
            } else {
                err
            }
        })
        .ok()
    } else {
        None
    };

    Ok((validate_call_info, execute_call_info))
}

#[instrument(name = "post_exec", skip_all, fields(
    tx_version = %args.transaction_version.0,
    calldata_len,
    signature_len,
))]
pub fn handle_post_exec_and_collect_gas_vectors(
    args: &SimulationArgs,
    cached_fork_state: &mut CachedState<ForkStateReader>,
    cheatnet_state: &mut CheatnetState,
    transaction_context: Arc<TransactionContext>,
    validate_call_info: &Option<CallInfo>,
    execute_call_info: &Option<CallInfo>,
    signature_len: usize,
    calldata_len: usize,
) -> Result<PostExecStateData, TransactionSimulationError> {
    let mut state_changes = cached_fork_state.to_state_diff()?;

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

    let mut tx_receipt = create_transaction_receipt(&transaction_context, &tx_resources);

    let post_exec_report =
        PostExecutionReport::new(cached_fork_state, &transaction_context, &tx_receipt, true)?;

    let is_revertable =
        is_transaction_revertible(args.transaction_type.unwrap(), &args.transaction_version);

    if args.transaction_version == TransactionVersion::THREE {
        let fee = tx_receipt.fee;
        let block_number = cached_fork_state.state.block_number();
        execute_fee_transfer(
            cached_fork_state,
            cheatnet_state,
            transaction_context.clone(),
            fee,
            versioned_constants
                .os_constants
                .gas_costs
                .base
                .default_initial_gas_cost,
            is_revertable,
            block_number,
        )?;

        let fee_state_changes = cached_fork_state.to_state_diff()?;

        let tx_resources = calculate_transaction_resources(
            args,
            &transaction_context,
            &execution_summary,
            signature_len,
            calldata_len,
            &fee_state_changes,
        );

        tx_receipt = create_transaction_receipt(&transaction_context, &tx_resources);
        state_changes = fee_state_changes;
    }

    let starknet_resources = tx_receipt.resources.starknet_resources;
    let computation_resources = tx_receipt.resources.computation;

    let (
        starknet_resources_gas_vector,
        starknet_resources_archival_data_gas_vector,
        starknet_resources_message_gas_vector,
        starknet_resources_state_gas_vector,
        computation_resources_gas_vector,
        computation_resources_vm_cost_gas_vector,
        computation_resources_sierra_gas_vector,
    ) = extract_gas_vectors(
        &starknet_resources,
        &computation_resources,
        versioned_constants,
        &gas_mode,
        use_kzg_da,
    );

    let estimated_fee = format_fee_string(tx_receipt.fee, transaction_context.tx_info.fee_type());

    let detailed_receipt = DetailedTransactionReceipt {
        fee: tx_receipt.fee,
        estimated_fee,

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

    let post_exec_state_data = PostExecStateData {
        state_changes,
        starknet_resources_state: starknet_resources.state.clone(),
        post_exec_report,
        detailed_receipt,
    };

    Ok(post_exec_state_data)
}

fn extract_gas_vectors(
    starknet: &StarknetResources,
    computation: &ComputationResources,
    constants: &VersionedConstants,
    gas_mode: &GasVectorComputationMode,
    use_kzg_da: bool,
) -> (
    GasVector, // total_starknet
    GasVector, // archival
    GasVector, // message
    GasVector, // state
    GasVector, // computation_total
    GasVector, // computation_vm
    GasVector, // computation_sierra
) {
    let total_starknet = starknet.to_gas_vector(constants, use_kzg_da, gas_mode);
    let archival = starknet.archival_data.to_gas_vector(constants, gas_mode);
    let message = starknet.messages.to_gas_vector();
    let state = starknet
        .state
        .to_gas_vector(use_kzg_da, &constants.allocation_cost);

    let computation_vm = get_extended_vm_resources_cost(
        constants,
        &computation.tx_extended_vm_resources,
        computation.n_reverted_steps,
        gas_mode,
    );

    let total_sierra_gas = computation
        .sierra_gas
        .checked_add(computation.reverted_sierra_gas)
        .unwrap_or_default();

    let sierra_gas = match gas_mode {
        GasVectorComputationMode::All => GasVector::from_l2_gas(total_sierra_gas),
        GasVectorComputationMode::NoL2Gas => {
            GasVector::from_l1_gas(constants.sierra_gas_to_l1_gas_amount_round_up(total_sierra_gas))
        }
    };

    let computation_total = computation.to_gas_vector(constants, gas_mode);

    (
        total_starknet,
        archival,
        message,
        state,
        computation_total,
        computation_vm,
        sierra_gas,
    )
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
        false,
    );

    let addition_os_resources = versioned_constants
        .get_additional_os_tx_resources(
            TransactionType::InvokeFunction,
            &starknet_resources,
            use_kzg_da,
        )
        .filter_unused_builtins();
    let total_vm_resources = &charged_resources.extended_vm_resources + &addition_os_resources;

    let computation_resources = ComputationResources {
        tx_extended_vm_resources: total_vm_resources,
        // TODO: add os_vm_resources
        os_vm_resources: Default::default(),
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

    let fee = transaction_context.tx_info.get_fee_by_gas_vector(
        transaction_context.block_context.block_info(),
        gas,
        Default::default(),
    );

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

fn get_entrypoint_selector() -> EntryPointSelector {
    selector_from_name(constants::EXECUTE_ENTRY_POINT_NAME)
}

fn is_transaction_revertible(
    transaction_type: TransactionType,
    transaction_version: &TransactionVersion,
) -> bool {
    match transaction_type {
        TransactionType::InvokeFunction => *transaction_version != TransactionVersion::ZERO,
        _ => false,
    }
}

pub fn get_execution_result(
    contract_calls_map: &HashMap<u32, ContractCall>,
    deepest_contract_call_id: Option<u32>,
    post_execution_report: Option<&PostExecutionReport>,
    execution_result: Option<ExecutionResult>,
) -> Result<ExecutionResult, TransactionSimulationError> {
    if post_execution_report.is_some() && deepest_contract_call_id.is_none() {
        if let Some(ExecutionResult::Reverted { reason }) = execution_result {
            return Ok(ExecutionResult::Reverted { reason });
        }
    }
    if let Some(deepest_contract_call_id) = deepest_contract_call_id {
        if let Some(call) = contract_calls_map.get(&deepest_contract_call_id) {
            if let Err(failure) = &call.result {
                match failure {
                    CallFailure::Recoverable { panic_data } => {
                        let reason = felts_to_string(panic_data.as_slice());
                        Ok(ExecutionResult::Reverted {
                            reason: reason.trim().to_string(),
                        })
                    }
                    CallFailure::Unrecoverable { msg } => {
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

#[instrument(name = "fee_transfer", skip_all, fields(actual_fee = %actual_fee.0, block_number))]
fn execute_fee_transfer(
    state: &mut dyn State,
    cheatnet_state: &mut CheatnetState,
    tx_context: Arc<TransactionContext>,
    actual_fee: Fee,
    initial_gas: u64,
    _is_revertable: bool,
    block_number: u64,
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

    let mut remaining_gas = initial_gas;
    let execution_result = execute_call_entry_point(
        &mut fee_transfer_call,
        state,
        cheatnet_state,
        &mut execution_context,
        &mut remaining_gas,
        &ExecuteCallEntryPointExtraOptions {
            trace_data_handled_by_revert_call: false,
        },
    )
    .map_err(|err: EntryPointExecutionError| {
        convert_entry_point_error_with_block(err, block_number)
    })?;

    Ok(execution_result)
}
