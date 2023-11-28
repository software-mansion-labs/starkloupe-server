use serde::Serialize;
use starknet_api::{
    core::{ClassHash, ContractAddress, EntryPointSelector, EthAddress},
    deprecated_contract_class::EntryPointType,
    hash::StarkFelt,
    transaction::{Calldata, EventContent, L2ToL1Payload},
};

#[derive(Serialize)]
pub struct TransactionExecutionInfo {
    /// Transaction validation call info; [None] for `L1Handler`.
    pub validate_call_info: Option<CallInfo>,
    /// Transaction execution call info; [None] for `Declare`.
    pub execute_call_info: Option<CallInfo>,
    /// Fee transfer call info; [None] for `L1Handler`.
    pub fee_transfer_call_info: Option<CallInfo>,
    /// TODO: Enable Fee when used
    /// The actual fee that was charged (in Wei).
    // pub actual_fee: Fee,
    /// TODO: Enable ResourcesMapping when used
    /// Actual execution resources the transaction is charged for,
    /// including L1 gas and additional OS resources estimation.
    // pub actual_resources: ResourcesMapping,
    /// Error string for reverted transactions; [None] if transaction execution was successful.
    // TODO(Dori, 1/8/2023): If the `Eq` and `PartialEq` traits are removed, or implemented on all
    //   internal structs in this enum, this field should be `Option<TransactionExecutionError>`.
    pub revert_error: Option<String>,
}

impl From<blockifier::transaction::objects::TransactionExecutionInfo> for TransactionExecutionInfo {
    fn from(result: blockifier::transaction::objects::TransactionExecutionInfo) -> Self {
        Self {
            validate_call_info: result.validate_call_info.map(Into::into),
            execute_call_info: result.execute_call_info.map(Into::into),
            fee_transfer_call_info: result.fee_transfer_call_info.map(Into::into),
            revert_error: result.revert_error.map(|error| error.to_string()),
        }
    }
}

#[derive(Serialize)]
pub struct CallInfo {
    pub call: CallEntryPoint,
    pub execution: CallExecution,
    // TODO: Enable vm_resources when used
    // pub vm_resources: VmExecutionResources,
    pub inner_calls: Vec<CallInfo>,
    // TODO: Enable storage_read_values and accessed_storage_keys when used
    // Additional information gathered during execution.
    // pub storage_read_values: Vec<StarkFelt>,
    // pub accessed_storage_keys: HashSet<StorageKey>,
}
impl From<blockifier::execution::call_info::CallInfo> for CallInfo {
    fn from(result: blockifier::execution::call_info::CallInfo) -> Self {
        Self {
            call: result.call.into(),
            execution: result.execution.into(),
            inner_calls: result.inner_calls.into_iter().map(Into::into).collect(),
        }
    }
}

#[derive(Serialize)]
pub struct CallEntryPoint {
    // The class hash is not given if it can be deduced from the storage address.
    pub class_hash: Option<ClassHash>,
    // Optional, since there is no address to the code implementation in a library call.
    // and for outermost calls (triggered by the transaction itself).
    // TODO: BACKWARD-COMPATIBILITY.
    pub code_address: Option<ContractAddress>,
    pub entry_point_type: EntryPointType,
    pub entry_point_selector: EntryPointSelector,
    pub calldata: Calldata,
    pub storage_address: ContractAddress,
    pub caller_address: ContractAddress,
    pub call_type: CallType,
    // We can assume that the initial gas is less than 2^64.
    pub initial_gas: u64,
}
impl From<blockifier::execution::entry_point::CallEntryPoint> for CallEntryPoint {
    fn from(result: blockifier::execution::entry_point::CallEntryPoint) -> Self {
        Self {
            class_hash: result.class_hash.map(Into::into),
            code_address: result.code_address.map(Into::into),
            entry_point_type: result.entry_point_type.into(),
            entry_point_selector: result.entry_point_selector.into(),
            calldata: result.calldata.into(),
            storage_address: result.storage_address.into(),
            caller_address: result.caller_address.into(),
            call_type: result.call_type.into(),
            initial_gas: result.initial_gas,
        }
    }
}

#[derive(Serialize)]
pub struct CallExecution {
    pub retdata: Retdata,
    pub events: Vec<OrderedEvent>,
    pub l2_to_l1_messages: Vec<OrderedL2ToL1Message>,
    pub failed: bool,
    pub gas_consumed: u64,
}
impl From<blockifier::execution::call_info::CallExecution> for CallExecution {
    fn from(result: blockifier::execution::call_info::CallExecution) -> Self {
        Self {
            retdata: result.retdata.into(),
            events: result.events.into_iter().map(Into::into).collect(),
            l2_to_l1_messages: result
                .l2_to_l1_messages
                .into_iter()
                .map(Into::into)
                .collect(),
            failed: result.failed,
            gas_consumed: result.gas_consumed,
        }
    }
}

#[derive(Serialize)]
pub struct Retdata(pub Vec<StarkFelt>);
impl From<blockifier::execution::call_info::Retdata> for Retdata {
    fn from(result: blockifier::execution::call_info::Retdata) -> Self {
        Self(result.0)
    }
}

#[derive(Serialize)]
pub struct OrderedEvent {
    pub order: usize,
    pub event: EventContent,
}
impl From<blockifier::execution::call_info::OrderedEvent> for OrderedEvent {
    fn from(result: blockifier::execution::call_info::OrderedEvent) -> Self {
        Self {
            order: result.order,
            event: result.event.into(),
        }
    }
}

#[derive(Serialize)]
pub struct MessageToL1 {
    pub to_address: EthAddress,
    pub payload: L2ToL1Payload,
}
impl From<blockifier::execution::call_info::MessageToL1> for MessageToL1 {
    fn from(result: blockifier::execution::call_info::MessageToL1) -> Self {
        Self {
            to_address: result.to_address.into(),
            payload: result.payload.into(),
        }
    }
}

#[derive(Serialize)]
pub struct OrderedL2ToL1Message {
    pub order: usize,
    pub message: MessageToL1,
}
impl From<blockifier::execution::call_info::OrderedL2ToL1Message> for OrderedL2ToL1Message {
    fn from(result: blockifier::execution::call_info::OrderedL2ToL1Message) -> Self {
        Self {
            order: result.order,
            message: result.message.into(),
        }
    }
}

#[derive(Default, Serialize)]
pub enum CallType {
    #[default]
    Call = 0,
    Delegate = 1,
}

impl From<blockifier::execution::entry_point::CallType> for CallType {
    fn from(result: blockifier::execution::entry_point::CallType) -> Self {
        match result {
            blockifier::execution::entry_point::CallType::Call => Self::Call,
            blockifier::execution::entry_point::CallType::Delegate => Self::Delegate,
        }
    }
}
