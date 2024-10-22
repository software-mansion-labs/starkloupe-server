pub mod build_debugger_data;
pub mod call_trace;
pub mod debugger_data_fetcher;
pub mod function_call;
pub mod function_calls_map;
pub mod mappings;
pub mod utils;

use cairo_lang_starknet_classes::contract_class::ContractClass;
use call_trace::DebuggerTraceEntry;
use serde::Serialize;
use std::collections::HashMap;
use verification::SierraStatementToCairoDebugInfo;

/// Contains the debugger data for all classes in a simulation
#[derive(Debug, Serialize)]
pub struct SimulationDebuggerData {
    pub classes_debugger_data: HashMap<String, ClassDebuggerData>,
    pub debugger_trace: Vec<DebuggerTraceEntry>,
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
    pub execution_trace: Vec<DebuggerTraceEntry>,
}
