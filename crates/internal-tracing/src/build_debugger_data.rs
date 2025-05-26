use crate::{
    call_trace::{get_internal_call_trace, get_simple_internal_call_trace},
    event_calls_map::EventCallsMap,
    function_calls_map::FunctionCallsMap,
    mappings::Mappings,
    ClassDataProvider, ClassDebuggerData, ClassDebuggerDataWithContractClass,
    ContractCallDebuggerData, DataWithContractClass,
};
use anyhow::Result;
use cairo_lang_sierra_to_casm::compiler::CairoProgram;
use cairo_vm::vm::trace::trace_entry::RelocatedTraceEntry;
use starknet::core::types::Felt;
use std::collections::HashMap;
use tracing::info;

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

pub enum ContractCallBuildResult {
    Simple {
        root_function_call_id: u32,
        panic_id: Option<u32>,
    },
    Full {
        debugger_data: ContractCallDebuggerData,
        root_function_call_id: u32,
        panic_id: Option<u32>,
    },
}

pub fn build_contract_call_debugger_data_adapter(
    vm_memory: &[Option<Felt>],
    vm_trace: &Vec<RelocatedTraceEntry>,
    full_class_debugger_data: &ClassDebuggerDataWithContractClass,
    function_calls_map: &mut FunctionCallsMap,
    _event_calls_map: &mut EventCallsMap,
    next_call_id: &mut u32,
    contract_call_id: u32,
    contract_call_children_ids: &[u32],
    casm_program: CairoProgram,
) -> Result<ContractCallBuildResult> {
    let (debugger_data, root_function_call_id, panic_id) = build_contract_call_debugger_data(
        vm_memory,
        vm_trace,
        full_class_debugger_data,
        function_calls_map,
        next_call_id,
        contract_call_id,
        contract_call_children_ids,
        casm_program,
    )?;
    Ok(ContractCallBuildResult::Full {
        debugger_data,
        root_function_call_id,
        panic_id,
    })
}

pub fn build_simple_contract_call_debugger_data_adapter(
    vm_memory: &[Option<Felt>],
    vm_trace: &Vec<RelocatedTraceEntry>,
    full_class_data: &DataWithContractClass,
    function_calls_map: &mut FunctionCallsMap,
    event_calls_map: &mut EventCallsMap,
    next_call_id: &mut u32,
    contract_call_id: u32,
    contract_call_children_ids: &[u32],
    casm_program: CairoProgram,
) -> Result<ContractCallBuildResult> {
    let (root_function_call_id, panic_id) = build_simple_contract_call_debugger_data(
        vm_memory,
        vm_trace,
        full_class_data,
        function_calls_map,
        event_calls_map,
        next_call_id,
        contract_call_id,
        contract_call_children_ids,
        casm_program,
    )?;

    Ok(ContractCallBuildResult::Simple {
        root_function_call_id,
        panic_id,
    })
}

// Return internal trace
fn build_simple_contract_call_debugger_data(
    vm_memory: &[Option<Felt>],
    vm_trace: &Vec<RelocatedTraceEntry>,
    full_class_data: &impl ClassDataProvider,
    function_calls_map: &mut FunctionCallsMap,
    event_calls_map: &mut EventCallsMap,
    next_call_id: &mut u32,
    contract_call_id: u32,
    contract_call_children_ids: &[u32],
    casm_program: CairoProgram,
) -> Result<(u32, Option<u32>)> {
    let mappings = Mappings::new(
        vm_trace,
        vm_memory,
        full_class_data.get_contract_class(),
        casm_program,
    )
    .map_err(|e| {
        info!("Failed to create mappings: {:?}", e);
        e
    })?;

    let (root_function_call_id, contract_call_id_with_panic_function_call) =
        get_simple_internal_call_trace(
            &mappings,
            vm_memory,
            vm_trace,
            function_calls_map,
            event_calls_map,
            next_call_id,
            contract_call_id,
            contract_call_children_ids,
        )?;

    Ok((
        root_function_call_id,
        contract_call_id_with_panic_function_call,
    ))
}

/// Returns the internal function call trace and sierra_execution_trace
fn build_contract_call_debugger_data(
    vm_memory: &[Option<Felt>],
    vm_trace: &Vec<RelocatedTraceEntry>,
    full_class_debugger_data: &impl ClassDataProvider,
    function_calls_map: &mut FunctionCallsMap,
    next_call_id: &mut u32,
    contract_call_id: u32,
    contract_call_children_ids: &[u32],
    casm_program: CairoProgram,
) -> Result<(ContractCallDebuggerData, u32, Option<u32>)> {
    let mappings = Mappings::new(
        vm_trace,
        vm_memory,
        full_class_debugger_data.get_contract_class(),
        casm_program,
    )
    .map_err(|e| {
        info!("Failed to create mappings: {:?}", e);
        e
    })?;

    let (execution_trace, root_function_call_id, contract_call_id_with_panic_function_call) =
        get_internal_call_trace(
            &mappings,
            vm_memory,
            vm_trace,
            full_class_debugger_data
                .get_debug_info()
                .map(|class_debugger_data| &class_debugger_data.sierra_statements_to_cairo_info),
            function_calls_map,
            next_call_id,
            contract_call_id,
            contract_call_children_ids,
        )?;

    Ok((
        ContractCallDebuggerData { execution_trace },
        root_function_call_id,
        contract_call_id_with_panic_function_call,
    ))
}
