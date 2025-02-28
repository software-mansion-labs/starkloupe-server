use cairo_lang_starknet_classes::abi::EventField;
use data_decoder::DecodedValue;
use serde::Serialize;

#[derive(Debug, Serialize)]
pub struct EventCall {
    pub call_id: u32,
    pub contract_call_id: u32,
    pub function_call_id: u32,

    pub name: String,
    pub selector: Option<String>,
    pub members: Vec<EventField>,
    pub datas: Option<Vec<DecodedValue>>,
    pub is_hidden: bool,
}
