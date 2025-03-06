pub mod calldata_decoder;
mod constants;
pub mod event_decoder;
pub mod internal_function_decoder;
mod starknet_types;
pub mod utils;
use num_traits::ToPrimitive;
use serde::ser::{Serialize, SerializeMap, SerializeStruct, Serializer};
use starknet_types_core::felt::Felt;
use std::{collections::HashMap, u128};

#[derive(Debug, Clone, Default)]
pub struct DecodedValue {
    pub name: Option<String>,
    pub type_name: String,
    pub value: DecodedValueType,
}

#[derive(Debug, Clone, Default)]
pub enum DecodedValueType {
    String(String),
    Single(Felt),
    Decimal(usize),
    Bool(bool),
    Array(Vec<DecodedValueType>),
    Struct(HashMap<usize, DecodedValue>),
    Enum(String, Box<DecodedValueType>),
    #[default]
    None,
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

pub fn create_decoded_value_by_type(
    name: Option<&str>,
    type_name: &str,
    value: DecodedValueType,
) -> DecodedValue {
    let value = match (type_name, &value) {
        ("bool", DecodedValueType::Single(felt)) => DecodedValueType::Bool(*felt != Felt::ZERO),
        ("u128", DecodedValueType::Single(felt))
        | ("u64", DecodedValueType::Single(felt))
        | ("u32", DecodedValueType::Single(felt))
        | ("u16", DecodedValueType::Single(felt))
        | ("u8", DecodedValueType::Single(felt)) => match felt.to_usize() {
            Some(num) => DecodedValueType::Decimal(num),
            None => DecodedValueType::Single(*felt),
        },
        _ => value,
    };
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
            DecodedValueType::Decimal(value) => value.serialize(serializer),
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
            DecodedValueType::Enum(variant_name, value) => {
                let mut map = serializer.serialize_map(Some(1))?;
                map.serialize_entry(variant_name, value)?;
                map.end()
            }
            DecodedValueType::None => serializer.serialize_none(),
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
