use serde_json::{json, map::Map, Value};

pub const SKIP_BUILTIN_TYPES: &[&str] = &[
    "Const",
    "Step",
    "Hole",
    "GasBuiltin",
    "ContractState",
    "ComponentState",
    "Bitwise",
    "BuiltinCosts",
    "EcOp",
    "RangeCheck",
    "SegmentArena",
    "Poseidon",
    "Pedersen",
    "RangeCheck96",
    "CircuitAdd",
    "CircuitMul",
    "Gas",
    "System",
    "()",
];

pub fn create_result_obj(
    names: &[String],
    index: usize,
    data_type: &str,
    value: Value,
) -> Map<String, Value> {
    let mut result_obj = Map::new();
    if !names.is_empty() && !names[index].is_empty() {
        result_obj.insert("name".to_string(), json!(names[index]));
    }
    result_obj.insert("type".to_string(), json!(data_type));
    result_obj.insert("value".to_string(), value);
    result_obj
}
