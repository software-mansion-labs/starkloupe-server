use crate::event_call::EventCall;
use serde::Serialize;
use std::collections::HashMap;

#[derive(Debug, Serialize, Default)]
pub struct EventCallsMap(pub HashMap<u32, EventCall>);
