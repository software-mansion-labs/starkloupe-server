use serde::ser::{Serialize, SerializeMap, SerializeStruct, Serializer};
use starknet_types_core::felt::Felt;
use std::collections::HashMap;

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

#[derive(Debug, Clone)]
pub struct DecodedValue {
    pub name: Option<String>,
    pub type_name: String,
    pub value: DecodedValueType,
}

#[derive(Debug, Clone)]
pub enum DecodedValueType {
    String(String),
    Single(Felt),
    Bool(bool),
    Array(Vec<DecodedValueType>),
    Struct(HashMap<usize, DecodedValue>),
    Enum(Box<DecodedValue>),
}

pub fn create_decoded_value(
    name: Option<&str>,
    type_name: &str,
    value: DecodedValueType,
) -> DecodedValue {
    DecodedValue {
        name: name.map(|s| s.to_string()),
        type_name: type_name.to_string(),
        value,
    }
}

impl Serialize for DecodedValueType {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        match self {
            DecodedValueType::String(value) => serializer.serialize_str(value),
            DecodedValueType::Single(value) => value.serialize(serializer),
            DecodedValueType::Bool(value) => serializer.serialize_bool(*value),
            DecodedValueType::Array(values) => values.serialize(serializer),
            DecodedValueType::Struct(fields) => {
                let mut map = serializer.serialize_map(Some(fields.len()))?;
                for (key, decoded_value) in fields {
                    let field_key = key.to_string();

                    map.serialize_entry(&field_key, &decoded_value)?;
                }
                map.end()
            }
            DecodedValueType::Enum(value) => value.serialize(serializer),
        }
    }
}

impl Serialize for DecodedValue {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let mut state = serializer.serialize_struct("DecodedValue", 3)?;
        state.serialize_field("name", &self.name)?;
        state.serialize_field("type_name", &self.type_name)?;
        state.serialize_field("value", &self.value)?;
        state.end()
    }
}

#[inline(always)]
pub fn simplify_type_name(type_str: &str) -> String {
    let inner_content = type_str
        .strip_prefix("Tuple<")
        .and_then(|s| s.strip_suffix('>'))
        .or_else(|| type_str.strip_prefix('(').and_then(|s| s.strip_suffix(')')))
        .unwrap_or(type_str);

    let mut parsed_types = Vec::with_capacity(inner_content.matches(", ").count() + 1);

    let mut current = String::new();
    let mut bracket_depth = 0;

    for c in inner_content.chars() {
        match c {
            '<' => {
                bracket_depth += 1;
                current.push(c);
            }
            '>' => {
                bracket_depth -= 1;
                current.push(c);
            }
            ',' if bracket_depth == 0 => {
                parsed_types.push(simplify_single_type(&current));
                current.clear();
            }
            _ => {
                current.push(c);
            }
        }
    }

    if !current.is_empty() {
        parsed_types.push(simplify_single_type(&current));
    }

    if parsed_types.len() > 1 {
        format!("({})", parsed_types.join(", "))
    } else {
        parsed_types.first().unwrap_or(&String::new()).to_string()
    }
}

#[inline(always)]
fn simplify_single_type(t: &str) -> String {
    if let Some(first_angle_bracket) = t.find('<') {
        let main_type = &t[..first_angle_bracket];
        let inner_type = &t[first_angle_bracket + 1..t.len() - 1];
        format_generic_type(main_type, inner_type)
    } else {
        extract_last_segment(t).to_string()
    }
}

#[inline(always)]
fn format_generic_type(main_type: &str, inner_type: &str) -> String {
    let main_type_name = extract_last_segment(main_type);
    let parsed_inner_type = simplify_type_name(inner_type);
    format!("{}<{}>", main_type_name, parsed_inner_type)
}

#[inline(always)]
fn extract_last_segment(t: &str) -> String {
    t.rsplit("::")
        .find(|&part| !part.is_empty())
        .unwrap_or(t)
        .to_string()
}

pub fn skip_builtin_type_declaration(type_name: &str) -> bool {
    SKIP_BUILTIN_TYPES
        .iter()
        .any(|&builtin| type_name.starts_with(builtin))
}
