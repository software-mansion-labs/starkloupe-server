use data_decoder::DecodedValue;
use serde::Serialize;
use walnut_shared::Parameter;

#[derive(Debug, Serialize)]
pub struct EventCall {
    pub call_id: u32,
    pub contract_call_id: u32,

    pub name: String,
    pub selector: String,
    pub datas: Option<Vec<DecodedValue>>,

    pub is_hidden: bool,
}
