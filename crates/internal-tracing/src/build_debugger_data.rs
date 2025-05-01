use anyhow::Result;
use cairo_lang_sierra_to_casm::compiler::CairoProgram;
use cairo_vm::vm::trace::trace_entry::RelocatedTraceEntry;
use starknet::core::types::Felt;
use std::collections::HashMap;
use tracing::info;

use crate::{
    call_trace::get_internal_call_trace, event_calls_map::EventCallsMap,
    function_calls_map::FunctionCallsMap, mappings::Mappings, ClassDebuggerData,
    ClassDebuggerDataWithContractClass, ContractCallDebuggerData,
};

pub fn debugger_data_maps_full_class_to_class(
    full_class_debugger_data_map: HashMap<String, ClassDebuggerDataWithContractClass>,
) -> HashMap<String, ClassDebuggerData> {
    full_class_debugger_data_map
        .into_iter()
        .filter_map(|(key, value)| {
            value
                .class_debugger_data
                .map(|class_debugger_data| (key, class_debugger_data))
        })
        .collect()
}

/// Returns the internal function call trace and sierra_execution_trace
pub fn build_contract_call_debugger_data(
    vm_memory: &[Option<Felt>],
    vm_trace: &Vec<RelocatedTraceEntry>,
    full_class_debugger_data: &ClassDebuggerDataWithContractClass,
    function_calls_map: &mut FunctionCallsMap,
    event_calls_map: &mut EventCallsMap,
    next_call_id: &mut u32,
    contract_call_id: u32,
    contract_call_children_ids: &[u32],
    casm_program: CairoProgram,
) -> Result<(ContractCallDebuggerData, u32)> {
    let mappings = Mappings::new(
        vm_trace,
        vm_memory,
        full_class_debugger_data.contract_class.clone(),
        casm_program,
    )
    .map_err(|e| {
        info!("Failed to create mappings: {:?}", e);
        e
    })?;

    let (execution_trace, root_function_call_id) = get_internal_call_trace(
        &mappings,
        vm_memory,
        vm_trace,
        full_class_debugger_data
            .class_debugger_data
            .as_ref()
            .map(|class_debugger_data| &class_debugger_data.sierra_statements_to_cairo_info),
        function_calls_map,
        event_calls_map,
        next_call_id,
        contract_call_id,
        contract_call_children_ids,
    )?;

    Ok((
        ContractCallDebuggerData { execution_trace },
        root_function_call_id,
    ))
}
