use blockifier::execution::entry_point::CallEntryPoint;
use cairo_vm::vm::trace::trace_entry::RelocatedTraceEntry;
use cheatnet::runtime_extensions::call_to_blockifier_runtime_extension::rpc::CallResult;
use cheatnet::state::CallTrace;
use data_decoder::{calldata_decoder::decode_calldata, DecodedValue};
use internal_tracing::ContractCallDebuggerData;
use serde::Serialize;
use starknet::core::types::Felt;
use std::borrow::Cow;
use std::cell::Ref;
use verification::CodeLocation;
use walnut_shared::abi::{Enum, Struct};
use walnut_shared::get_name_of_entry_point_selector;

#[derive(Debug, Serialize)]
pub struct ContractCall {
    pub call_id: u32,
    pub parent_call_id: u32,
    pub children_call_ids: Vec<u32>,
    pub function_call_id: Option<u32>,

    pub entry_point: CallEntryPoint,
    pub result: CallResult,
    pub vm_trace: Option<Vec<RelocatedTraceEntry>>,
    pub vm_memory: Option<Vec<Option<Felt>>>,

    pub contract_name: Option<String>,
    pub entry_point_name: Option<String>,
    pub entry_point_selector: Option<String>,
    pub entry_point_interface_name: Option<String>,
    pub is_erc20_token: bool,
    pub erc20_token_name: Option<String>,
    pub erc20_token_symbol: Option<String>,
    pub error_message: Option<String>,
    pub call_debugger_data: Option<ContractCallDebuggerData>,
    pub class_hash: Option<String>,
    pub sierra_version: Option<String>,
    pub cairo_version: Option<String>,
    pub is_failed: bool,
    pub is_deepest_panic_result: bool,

    pub result_types: Option<Vec<Cow<'static, str>>>,
    pub arguments_names: Option<Vec<Cow<'static, str>>>,
    pub arguments_types: Option<Vec<Cow<'static, str>>>,
    pub calldata_decoded: Option<Vec<DecodedValue>>,
    pub decoded_result: Option<Vec<DecodedValue>>,

    pub contract_calls_nesting_level: u32,

    pub code_location: Option<CodeLocation>,
    pub debugger_trace_step_index: Option<usize>,

    pub is_hidden: bool,
}

impl ContractCall {
    pub fn from_cheatnet_state_calltrace(
        call_trace_ref: &Ref<CallTrace>,
        call_id: u32,
        parent_call_id: u32,
        contract_calls_nesting_level: u32,
        is_hidden: bool,
    ) -> Self {
        let entry_point_name =
            get_name_of_entry_point_selector(&call_trace_ref.entry_point.entry_point_selector.0);

        ContractCall {
            call_id,
            parent_call_id,
            children_call_ids: Vec::new(),
            function_call_id: None,

            entry_point: call_trace_ref.entry_point.clone(),
            result: call_trace_ref.result.clone(),
            vm_trace: call_trace_ref.vm_trace.clone(),
            vm_memory: call_trace_ref.vm_memory.clone(),
            contract_name: None,
            entry_point_name,
            entry_point_selector: None,
            entry_point_interface_name: None,
            is_erc20_token: false,
            erc20_token_name: None,
            erc20_token_symbol: None,
            error_message: None,
            call_debugger_data: None,
            class_hash: call_trace_ref
                .entry_point
                .class_hash
                .map(|class_hash| class_hash.0.to_fixed_hex_string()),
            sierra_version: None,
            cairo_version: None,
            is_failed: matches!(call_trace_ref.result, CallResult::Failure(_)),
            is_deepest_panic_result: false,

            result_types: None,
            decoded_result: None,
            arguments_names: None,
            arguments_types: None,
            calldata_decoded: None,

            contract_calls_nesting_level,

            code_location: None,
            debugger_trace_step_index: None,

            is_hidden,
        }
    }

    /// Decodes the call arguments using the provided struct ABIs. The decoded arguments are then stored in the `calldata_decoded` field.
    ///
    /// Note: `arguments_types` and `arguments_names` should already be set before calling this function.
    pub fn decode_call_arguments(&mut self, struct_abis: &[Struct], enum_abis: &[Enum]) {
        if let (Some(arguments_types), Some(arguments_names)) =
            (&self.arguments_types, &self.arguments_names)
        {
            let decoded_arguments = decode_calldata(
                &self.entry_point.calldata.0.to_vec(),
                arguments_types,
                arguments_names,
                Some(struct_abis),
                Some(enum_abis),
                &mut 0,
            );

            self.calldata_decoded = decoded_arguments;
        }
    }

    /// Decodes the call result using the provided struct ABIs. The decoded result is then stored in the `decoded_result` field.
    ///
    /// Note: `result_types` should already be set before calling this function.
    pub fn decode_call_result(&mut self, struct_abis: &[Struct], enum_abis: &[Enum]) {
        if let CallResult::Success { ret_data } = &self.result {
            if let Some(call_result_types) = &self.result_types {
                let decoded_result = decode_calldata(
                    ret_data,
                    call_result_types,
                    &[],
                    Some(struct_abis),
                    Some(enum_abis),
                    &mut 0,
                );
                self.decoded_result = decoded_result;
            }
        }
    }
}
