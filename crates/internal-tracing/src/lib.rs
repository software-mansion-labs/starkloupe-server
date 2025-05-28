pub mod build_debugger_data;
pub mod call_trace;
pub mod debugger_data_fetcher;
pub mod event_call;
pub mod event_calls_map;
pub mod function_call;
pub mod function_calls_map;
pub mod mappings;
pub mod utils;
use cairo_lang_starknet_classes::contract_class::ContractClass;
use call_trace::DebuggerTraceEntry;
use serde::Serialize;
use std::collections::HashMap;
use thiserror::Error;
use verification::SierraStatementToCairoDebugInfo;

pub trait ClassDataProvider {
    fn get_contract_class(&self) -> &ContractClass;
    fn get_inline_strategy_class_hash(&self) -> Option<String>;
    fn get_debug_info(&self) -> Option<&ClassDebuggerData>;
}

impl ClassDataProvider for DataWithContractClass {
    fn get_contract_class(&self) -> &ContractClass {
        &self.contract_class
    }
    fn get_inline_strategy_class_hash(&self) -> Option<String> {
        self.inline_strategy_class_hash.clone()
    }
    fn get_debug_info(&self) -> Option<&ClassDebuggerData> {
        None
    }
}

impl ClassDataProvider for ClassDebuggerDataWithContractClass {
    fn get_contract_class(&self) -> &ContractClass {
        &self.contract_class
    }
    fn get_inline_strategy_class_hash(&self) -> Option<String> {
        self.inline_strategy_class_hash.clone()
    }
    fn get_debug_info(&self) -> Option<&ClassDebuggerData> {
        self.class_debugger_data.as_ref()
    }
}

#[derive(Error, Debug)]
pub enum TraceError {
    #[error("Mapping error {0}")]
    MappingError(String),
    #[error("Sotrage error {0}")]
    StorageError(String),
    #[error("Internal trace error {0}")]
    InternalTraceError(String),
    #[error("Compile contract class error {0}")]
    CompilationError(String),
}

/// Contains the debugger data for a class with the Sierra contract class
#[derive(Debug)]
pub struct DataWithContractClass {
    pub inline_strategy_class_hash: Option<String>,
    pub contract_class: ContractClass,
}

/// Contains the debugger data for all classes in a simulation
#[derive(Debug, Serialize)]
pub struct SimulationDebuggerData {
    pub classes_debugger_data: HashMap<String, ClassDebuggerData>,
    pub debugger_trace: Vec<DebuggerTraceEntry>,
}

/// Contains the debugger data for a class with the Sierra contract class
#[derive(Debug)]
pub struct ClassDebuggerDataWithContractClass {
    pub inline_strategy_class_hash: Option<String>,
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
