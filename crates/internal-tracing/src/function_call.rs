use data_decoder::DecodedValue;
use serde::Serialize;
use verification::CodeLocation;

use crate::call_trace::InternalFnCallIO;

#[derive(Debug, Serialize, Clone)]
pub struct FunctionCall {
    pub call_id: u32,
    pub parent_call_id: u32,
    pub children_call_ids: Vec<u32>,
    pub contract_call_id: u32,
    pub event_call_ids: Vec<u32>,

    pub fn_name: Option<String>,
    pub fp: usize,
    pub is_deepest_panic_result: bool,
    pub results: Vec<InternalFnCallIO>,
    pub results_decoded: Option<Vec<DecodedValue>>,
    pub arguments: Vec<InternalFnCallIO>,
    pub arguments_decoded: Option<Vec<DecodedValue>>,

    pub debugger_data_available: bool,
    pub debugger_trace_step_index: Option<usize>,
    pub code_location: Option<CodeLocation>,

    pub is_hidden: bool,
}
