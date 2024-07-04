pub mod call_trace;
pub mod debugger_data_fetcher;
pub mod mappings;
pub mod utils;
use crate::mappings::Mappings;

use anyhow::Result;
use cairo_felt::Felt252;
use cairo_lang_starknet_classes::contract_class::ContractClass;
use cairo_vm::vm::trace::trace_entry::TraceEntry;
use call_trace::{
    get_internal_call_trace, DebuggerExecutionTraceEntry, InternalFnCallTraceEntryNode,
};
use serde::Serialize;
use std::collections::HashMap;
use verification::cairo_debug_info::SierraStatementToCairoDebugInfo;

/// Contains the debugger data for all classes in a simulation
#[derive(Debug, Serialize)]
pub struct SimulationDebuggerData {
    pub classes_debugger_data: HashMap<String, ClassDebuggerData>,
}

/// Contains the debugger data for a class with the Sierra contract class
#[derive(Debug)]
pub struct ClassDebuggerDataWithContractClass {
    pub class_debugger_data: Option<ClassDebuggerData>,
    pub contract_class: ContractClass,
}

/// Contains the debugger data for a class
#[derive(Debug, Serialize)]
pub struct ClassDebuggerData {
    pub sierra_statements_to_cairo_info: HashMap<usize, SierraStatementToCairoDebugInfo>,
    pub source_code: HashMap<String, String>,
}

/// Contains the debugger data for a contract call
#[derive(Debug, Serialize)]
pub struct ContractCallDebuggerData {
    pub execution_trace: Vec<DebuggerExecutionTraceEntry>,
}

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
pub fn get_internal_trace_and_debugger_data(
    relocated_memory: &Vec<Option<Felt252>>,
    vm_trace: &Vec<TraceEntry>,
    full_class_debugger_data: &ClassDebuggerDataWithContractClass,
) -> Result<(InternalFnCallTraceEntryNode, ContractCallDebuggerData)> {
    let mappings = Mappings::new(
        relocated_memory,
        vm_trace,
        full_class_debugger_data.contract_class.clone(),
    )?;

    let (internal_trace, execution_trace) = get_internal_call_trace(
        &mappings,
        relocated_memory,
        vm_trace,
        full_class_debugger_data
            .class_debugger_data
            .as_ref()
            .map(|class_debugger_data| &class_debugger_data.sierra_statements_to_cairo_info),
    )?;

    Ok((internal_trace, ContractCallDebuggerData { execution_trace }))
}
