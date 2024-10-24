use serde::Serialize;
use std::collections::HashMap;

use crate::function_call::FunctionCall;

#[derive(Debug, Serialize)]
pub struct FunctionCallsMap(pub HashMap<u32, FunctionCall>);

impl FunctionCallsMap {
    pub fn new() -> Self {
        FunctionCallsMap(HashMap::new())
    }
}
