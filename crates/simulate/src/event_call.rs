use serde::Serialize;
use verification::CodeLocation;
use walnut_shared::Parameter;

#[derive(Debug, Serialize)]
pub struct EventCall {
    pub call_id: u32,
    pub contract_call_id: u32,

    pub name: String,
    pub keys: Vec<String>,
    pub parameters: Vec<Parameter>,
    pub data: Vec<String>,

    pub debugger_trace_step_index: Option<usize>,
    pub code_location: Option<CodeLocation>,

    pub is_hidden: bool,
}
