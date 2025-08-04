pub mod calldata_decoder;
mod constants;
pub mod event_decoder;
pub mod internal_function_decoder;
mod starknet_types;
pub mod utils;
use num_bigint::{BigInt, BigUint};
use num_traits::One;
use serde::ser::{Serialize, SerializeMap, SerializeStruct, Serializer};
use starknet_types_core::felt::Felt;
use std::collections::HashMap;

//A negative value -x is serialized as P - x, where P is:
//P = 2^251 + 17 * 2^192 + 1
//https://docs.starknet.io/architecture-and-concepts/cryptography/#stark-field
//https://docs.starknet.io/architecture-and-concepts/smart-contracts/serialization-of-cairo-types/#serialization_of_unsigned_integers
lazy_static::lazy_static! {
    static ref P: BigInt = {
        let two = BigInt::from(2);
        two.pow(251) + BigInt::from(17) * two.pow(192) + BigInt::one()
    };
}

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
    BigUint(BigUint),
    BigInt(BigInt),
    Bool(bool),
    Array(Vec<DecodedValueType>),
    Struct(HashMap<usize, DecodedValue>),
    Enum(String, Box<DecodedValue>),
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
        // integer type cairo
        ("u128", DecodedValueType::Single(felt))
        | ("u64", DecodedValueType::Single(felt))
        | ("u32", DecodedValueType::Single(felt))
        | ("u16", DecodedValueType::Single(felt))
        | ("u8", DecodedValueType::Single(felt)) => DecodedValueType::BigUint(felt.to_biguint()),
        ("i128", DecodedValueType::Single(felt))
        | ("i64", DecodedValueType::Single(felt))
        | ("i32", DecodedValueType::Single(felt))
        | ("i16", DecodedValueType::Single(felt))
        | ("i8", DecodedValueType::Single(felt)) => {
            let mut value = felt.to_bigint();
            value -= &*P;
            DecodedValueType::BigInt(value)
        }
        // u256 -> [low, high]
        // - It expects a structure with exactly two parts: "low" (lower 128-bit part) and "high" (upper 128-bit part).
        // - The upper part is shifted left by 128 bits (equivalent to multiplying by 2^128) to make room for the lower part.
        // - The values are then combined using the bitwise OR operator to reconstruct the full 256-bit number: u256 = (high << 128) | low.
        ("u256", DecodedValueType::Struct(values)) if values.len() == 2 => {
            let low = values.get(&0).and_then(|v| match &v.value {
                DecodedValueType::BigUint(low) => Some(low.clone()),
                _ => None,
            });

            let high = values.get(&1).and_then(|v| match &v.value {
                DecodedValueType::BigUint(high) => Some(high.clone()),
                _ => None,
            });

            if let (Some(low), Some(high)) = (low, high) {
                let u256_value = (high << 128) | low;
                DecodedValueType::BigUint(u256_value)
            } else {
                DecodedValueType::Struct(values.clone())
            }
        }
        // u512 -> [limb0, limb1, limb2, limb3]
        // - It expects a structure with four parts: limb0, limb1, limb2, and limb3, each representing 128 bits.
        // - Limb3 is shifted left by 384 bits, limb2 by 256 bits, limb1 by 128 bits, while limb0 remains in place.
        // - Combining all parts using the OR operator reconstructs the full 512-bit number: u512 = (limb3 << 384) | (limb2 << 256) | (limb1 << 128) | limb0.
        ("u512", DecodedValueType::Struct(values)) if values.len() == 4 => {
            let limb0 = values.get(&0).and_then(|v| match &v.value {
                DecodedValueType::BigUint(limb0) => Some(limb0.clone()),
                _ => None,
            });
            let limb1 = values.get(&1).and_then(|v| match &v.value {
                DecodedValueType::BigUint(limb1) => Some(limb1.clone()),
                _ => None,
            });
            let limb2 = values.get(&2).and_then(|v| match &v.value {
                DecodedValueType::BigUint(limb2) => Some(limb2.clone()),
                _ => None,
            });
            let limb3 = values.get(&3).and_then(|v| match &v.value {
                DecodedValueType::BigUint(limb3) => Some(limb3.clone()),
                _ => None,
            });

            if let (Some(limb0), Some(limb1), Some(limb2), Some(limb3)) =
                (limb0, limb1, limb2, limb3)
            {
                let u512_value = (limb3 << 384) | (limb2 << 256) | (limb1 << 128) | limb0;
                DecodedValueType::BigUint(u512_value)
            } else {
                DecodedValueType::Struct(values.clone())
            }
        }
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
            DecodedValueType::BigUint(value) => serializer.serialize_str(&value.to_str_radix(10)),
            DecodedValueType::BigInt(value) => serializer.serialize_str(&value.to_str_radix(10)),
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

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json;

    #[test]
    fn test_basic_decoded_value_creation() {
        let value = create_decoded_value(
            Some("test_field"),
            "felt252",
            DecodedValueType::String("0x123".to_string()),
        );

        assert_eq!(value.name, Some("test_field".to_string()));
        assert_eq!(value.type_name, "felt252");
        assert!(matches!(value.value, DecodedValueType::String(_)));
    }

    #[test]
    fn test_basic_serialization() {
        let value = DecodedValue {
            name: Some("test".to_string()),
            type_name: "felt252".to_string(),
            value: DecodedValueType::String("0x123".to_string()),
        };

        let json = serde_json::to_string(&value).unwrap();
        assert!(json.contains("\"name\":\"test\""));
        assert!(json.contains("\"type_name\":\"felt252\""));
        assert!(json.contains("\"0x123\""));
    }

    #[test]
    fn test_array_serialization() {
        let array_value = DecodedValueType::Array(vec![
            DecodedValueType::String("0x1".to_string()),
            DecodedValueType::String("0x2".to_string()),
        ]);

        let json = serde_json::to_string(&array_value).unwrap();
        assert!(json.contains("\"0x1\""));
        assert!(json.contains("\"0x2\""));
    }

    #[test]
    fn test_struct_serialization() {
        let struct_value = DecodedValueType::Struct({
            let mut map = HashMap::new();
            map.insert(
                0,
                DecodedValue {
                    name: None,
                    type_name: "field1".to_string(),
                    value: DecodedValueType::String("value1".to_string()),
                },
            );
            map.insert(
                1,
                DecodedValue {
                    name: None,
                    type_name: "field2".to_string(),
                    value: DecodedValueType::String("value2".to_string()),
                },
            );
            map
        });

        let json = serde_json::to_string(&struct_value).unwrap();
        assert!(json.contains("\"0\":"));
        assert!(json.contains("\"1\":"));
        assert!(json.contains("\"value1\""));
        assert!(json.contains("\"value2\""));
    }

    #[test]
    fn test_single_field_struct_unwrapping() {
        // Test that single-field structs are unwrapped
        let inner_value = DecodedValue {
            name: None,
            type_name: "inner_type".to_string(),
            value: DecodedValueType::String("inner_value".to_string()),
        };

        let struct_value = DecodedValueType::Struct({
            let mut map = HashMap::new();
            map.insert(0, inner_value);
            map
        });

        let json = serde_json::to_string(&struct_value).unwrap();
        // Should contain the "0" wrapper since it's a struct field
        assert!(json.contains("\"0\":"));
        // Should contain the inner value
        assert!(json.contains("\"type_name\":\"inner_type\""));
        assert!(json.contains("\"inner_value\""));
    }

    #[test]
    fn test_enum_serialization_with_some_variant() {
        // Test Option<T> with Some variant
        let inner_value = DecodedValue {
            name: None,
            type_name: "Array<Call>".to_string(),
            value: DecodedValueType::Array(vec![DecodedValueType::String("call1".to_string())]),
        };

        let enum_value = DecodedValueType::Enum("Some".to_string(), Box::new(inner_value));

        let json = serde_json::to_string(&enum_value).unwrap();
        // Should contain the "Some" wrapper since type_name is not "Some"
        assert!(json.contains("\"Some\":"));
        // Should contain the inner value
        assert!(json.contains("\"type_name\":\"Array<Call>\""));
        assert!(json.contains("\"call1\""));
    }

    #[test]
    fn test_enum_serialization_with_true_variant() {
        // Test true enum variant (should be wrapped)
        let inner_value = DecodedValue {
            name: None,
            type_name: "Error".to_string(),
            value: DecodedValueType::String("error_message".to_string()),
        };

        let enum_value = DecodedValueType::Enum("Error".to_string(), Box::new(inner_value));

        let json = serde_json::to_string(&enum_value).unwrap();
        // Should contain the variant name as wrapper
        assert!(json.contains("\"Error\":"));
    }

    #[test]
    fn test_none_variant_wrapping() {
        // Test Option<T> with None variant (should be wrapped)
        let inner_value = DecodedValue {
            name: None,
            type_name: "Unit".to_string(),
            value: DecodedValueType::None,
        };

        let enum_value = DecodedValueType::Enum("None".to_string(), Box::new(inner_value));

        let json = serde_json::to_string(&enum_value).unwrap();
        // Should contain the variant name as wrapper
        assert!(json.contains("\"None\":"));
    }

    #[test]
    fn test_complex_nested_structure() {
        // Test a complex nested structure similar to the user's example
        let call_struct = DecodedValueType::Struct({
            let mut map = HashMap::new();
            map.insert(
                0,
                DecodedValue {
                    name: None,
                    type_name: "ContractAddress".to_string(),
                    value: DecodedValueType::String("0x123".to_string()),
                },
            );
            map.insert(
                1,
                DecodedValue {
                    name: None,
                    type_name: "felt252".to_string(),
                    value: DecodedValueType::String("0x456".to_string()),
                },
            );
            map
        });

        let array_value = DecodedValueType::Array(vec![call_struct]);

        let option_inner = DecodedValue {
            name: None,
            type_name: "Array<Call>".to_string(),
            value: array_value,
        };

        let option_enum = DecodedValueType::Enum("Some".to_string(), Box::new(option_inner));

        let json = serde_json::to_string(&option_enum).unwrap();
        println!("Complex nested structure: {}", json);

        // Should contain the "Some" wrapper since type_name is not "Some"
        assert!(json.contains("\"Some\":"));
        // Should contain the inner structure
        assert!(json.contains("\"type_name\":\"Array<Call>\""));
        assert!(json.contains("\"0x123\""));
        assert!(json.contains("\"0x456\""));
    }
}

#[cfg(test)]
mod integration_tests {
    use super::*;
    use serde_json;

    #[test]
    fn test_enum_serialization_without_redundant_wrapping() {
        // Test case 1: Enum where variant name is "Some" (Option type)
        let inner_value = DecodedValue {
            name: None,
            type_name: "Array<Call>".to_string(),
            value: DecodedValueType::Array(vec![DecodedValueType::Struct({
                let mut map = HashMap::new();
                map.insert(
                    0,
                    DecodedValue {
                        name: None,
                        type_name: "ContractAddress".to_string(),
                        value: DecodedValueType::String(
                            "0x656cf823a768f7ca7f742778283393062708343f8961e4c418b38ac99857ce5"
                                .to_string(),
                        ),
                    },
                );
                map.insert(
                    1,
                    DecodedValue {
                        name: None,
                        type_name: "felt252".to_string(),
                        value: DecodedValueType::String(
                            "0x239e4c8fbd11b680d7214cfc26d1780d5c099453f0832beb15fd040aebd4ebb"
                                .to_string(),
                        ),
                    },
                );
                map.insert(
                    2,
                    DecodedValue {
                        name: None,
                        type_name: "Span<felt252>".to_string(),
                        value: DecodedValueType::Array(vec![DecodedValueType::String(
                            "0x1".to_string(),
                        )]),
                    },
                );
                map
            })]),
        };

        let enum_value = DecodedValueType::Enum("Some".to_string(), Box::new(inner_value));

        let json = serde_json::to_string(&enum_value).unwrap();
        println!("Serialized enum: {}", json);

        // The JSON should contain the "Some" wrapper since type_name is not "Some"
        assert!(json.contains("\"Some\":"));
        assert!(json.contains("\"type_name\":\"Array<Call>\""));
    }

    #[test]
    fn test_enum_serialization_with_true_enum_variant() {
        // Test case 2: True enum variant (should wrap)
        let inner_value = DecodedValue {
            name: None,
            type_name: "Some".to_string(),
            value: DecodedValueType::String("test_value".to_string()),
        };

        let enum_value = DecodedValueType::Enum("Some".to_string(), Box::new(inner_value));

        let json = serde_json::to_string(&enum_value).unwrap();
        println!("Serialized true enum: {}", json);

        // The JSON should contain the variant name as a wrapper
        assert!(json.contains("\"Some\":"));
    }

    #[test]
    fn test_enum_serialization_with_none_variant() {
        // Test case 3: None variant (should wrap)
        let inner_value = DecodedValue {
            name: None,
            type_name: "Unit".to_string(),
            value: DecodedValueType::None,
        };

        let enum_value = DecodedValueType::Enum("None".to_string(), Box::new(inner_value));

        let json = serde_json::to_string(&enum_value).unwrap();
        println!("Serialized None enum: {}", json);

        // The JSON should contain the variant name as a wrapper
        assert!(json.contains("\"None\":"));
    }

    #[test]
    fn test_struct_serialization_with_multiple_fields() {
        // Test case 5: Struct with multiple fields (should wrap)
        let struct_value = DecodedValueType::Struct({
            let mut map = HashMap::new();
            map.insert(
                0,
                DecodedValue {
                    name: None,
                    type_name: "field1".to_string(),
                    value: DecodedValueType::String("value1".to_string()),
                },
            );
            map.insert(
                1,
                DecodedValue {
                    name: None,
                    type_name: "field2".to_string(),
                    value: DecodedValueType::String("value2".to_string()),
                },
            );
            map
        });

        let json = serde_json::to_string(&struct_value).unwrap();
        println!("Serialized multi-field struct: {}", json);

        // The JSON should contain the field keys as wrappers
        assert!(json.contains("\"0\":"));
        assert!(json.contains("\"1\":"));
    }

    #[test]
    fn test_option_struct_serialization_without_redundant_wrapping() {
        // Test case 6: Option<T> decoded as struct with one field (should unwrap)
        let inner_value = DecodedValue {
            name: None,
            type_name: "Array<Call>".to_string(),
            value: DecodedValueType::Array(vec![DecodedValueType::Struct({
                let mut map = HashMap::new();
                map.insert(
                    0,
                    DecodedValue {
                        name: None,
                        type_name: "ContractAddress".to_string(),
                        value: DecodedValueType::String("0x123".to_string()),
                    },
                );
                map
            })]),
        };

        let option_struct = DecodedValueType::Struct({
            let mut map = HashMap::new();
            map.insert(0, inner_value);
            map
        });

        let json = serde_json::to_string(&option_struct).unwrap();
        println!("Serialized Option struct: {}", json);

        // The JSON should contain the "0" wrapper since it's a struct field
        assert!(json.contains("\"0\":"));
        assert!(json.contains("\"type_name\":\"Array<Call>\""));
    }
}
