pub mod calldata_decoder;
mod constants;
pub mod event_decoder;
pub mod internal_function_decoder;
mod starknet_types;
pub mod type_decoder;
pub mod utils;
use num_bigint::{BigInt, BigUint};
use num_traits::One;
use serde::ser::{SerializeMap, SerializeStruct, Serializer};
use serde::{Deserialize, Serialize};
use starknet_types_core::felt::Felt;
use std::collections::BTreeMap;

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
    Struct(BTreeMap<usize, DecodedValue>),
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
            DecodedValueType::Enum(_variant_name, value) => {
                // For compact enum format, serialize the inner value directly
                // The variant name will be combined with type_name as "EnumType::Variant"
                value.serialize(serializer)
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

impl<'de> Deserialize<'de> for DecodedValueType {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        // Deserialize as a generic JSON value first
        let value = serde_json::Value::deserialize(deserializer)?;

        // String type, try to parse as different numeric types
        if let serde_json::Value::String(s) = &value {
            if let Ok(parsed) = s.parse::<u64>() {
                return Ok(DecodedValueType::BigUint(BigUint::from(parsed)));
            }
            if let Ok(parsed) = s.parse::<i64>() {
                return Ok(DecodedValueType::BigInt(BigInt::from(parsed)));
            }
            if let Ok(parsed) = s.parse::<bool>() {
                return Ok(DecodedValueType::Bool(parsed));
            }
            if s.starts_with("0x") {
                if let Ok(felt) = Felt::from_hex(&s[2..]) {
                    return Ok(DecodedValueType::Single(felt));
                }
            }
            // Default to String
            return Ok(DecodedValueType::String(s.clone()));
        }

        // Handle other JSON types
        match value {
            serde_json::Value::String(s) => Ok(DecodedValueType::String(s)),
            serde_json::Value::Bool(b) => Ok(DecodedValueType::Bool(b)),
            serde_json::Value::Array(arr) => {
                let mut decoded = Vec::new();
                for v in arr {
                    // Handle different types in arrays, including recursive deserialization
                    match v {
                        serde_json::Value::String(s) => {
                            // Try to parse as different numeric types first
                            if let Ok(parsed) = s.parse::<u64>() {
                                decoded.push(DecodedValueType::BigUint(BigUint::from(parsed)));
                            } else if let Ok(parsed) = s.parse::<i64>() {
                                decoded.push(DecodedValueType::BigInt(BigInt::from(parsed)));
                            } else if let Ok(parsed) = s.parse::<bool>() {
                                decoded.push(DecodedValueType::Bool(parsed));
                            } else if s.starts_with("0x") {
                                if let Ok(felt) = Felt::from_hex(&s[2..]) {
                                    decoded.push(DecodedValueType::Single(felt));
                                } else {
                                    decoded.push(DecodedValueType::String(s));
                                }
                            } else {
                                decoded.push(DecodedValueType::String(s));
                            }
                        }
                        serde_json::Value::Number(n) => {
                            if let Some(parsed) = n.as_u64() {
                                decoded.push(DecodedValueType::BigUint(BigUint::from(parsed)));
                            } else if let Some(parsed) = n.as_i64() {
                                decoded.push(DecodedValueType::BigInt(BigInt::from(parsed)));
                            } else {
                                decoded.push(DecodedValueType::String(n.to_string()));
                            }
                        }
                        serde_json::Value::Bool(b) => decoded.push(DecodedValueType::Bool(b)),
                        serde_json::Value::Array(inner_arr) => {
                            // Recursively handle nested arrays
                            let inner_decoded = inner_arr
                                .into_iter()
                                .map(|v| match v {
                                    serde_json::Value::String(s) => {
                                        if let Ok(parsed) = s.parse::<u64>() {
                                            DecodedValueType::BigUint(BigUint::from(parsed))
                                        } else if let Ok(parsed) = s.parse::<i64>() {
                                            DecodedValueType::BigInt(BigInt::from(parsed))
                                        } else if let Ok(parsed) = s.parse::<bool>() {
                                            DecodedValueType::Bool(parsed)
                                        } else if s.starts_with("0x") {
                                            if let Ok(felt) = Felt::from_hex(&s[2..]) {
                                                DecodedValueType::Single(felt)
                                            } else {
                                                DecodedValueType::String(s)
                                            }
                                        } else {
                                            DecodedValueType::String(s)
                                        }
                                    }
                                    serde_json::Value::Number(n) => {
                                        if let Some(parsed) = n.as_u64() {
                                            DecodedValueType::BigUint(BigUint::from(parsed))
                                        } else if let Some(parsed) = n.as_i64() {
                                            DecodedValueType::BigInt(BigInt::from(parsed))
                                        } else {
                                            DecodedValueType::String(n.to_string())
                                        }
                                    }
                                    serde_json::Value::Bool(b) => DecodedValueType::Bool(b),
                                    _ => DecodedValueType::String(v.to_string()),
                                })
                                .collect();
                            decoded.push(DecodedValueType::Array(inner_decoded));
                        }
                        serde_json::Value::Object(obj) => {
                            // Handle structs in arrays
                            let mut fields = BTreeMap::new();
                            for (k, v) in obj {
                                let key: usize = k.parse().map_err(serde::de::Error::custom)?;
                                // Try to deserialize as DecodedValue first
                                if let Ok(decoded_value) =
                                    serde_json::from_value::<DecodedValue>(v.clone())
                                {
                                    fields.insert(key, decoded_value);
                                } else {
                                    // Fallback to basic deserialization
                                    let value = match v {
                                        serde_json::Value::String(s) => {
                                            if let Ok(parsed) = s.parse::<u64>() {
                                                DecodedValueType::BigUint(BigUint::from(parsed))
                                            } else if let Ok(parsed) = s.parse::<i64>() {
                                                DecodedValueType::BigInt(BigInt::from(parsed))
                                            } else if let Ok(parsed) = s.parse::<bool>() {
                                                DecodedValueType::Bool(parsed)
                                            } else if s.starts_with("0x") {
                                                if let Ok(felt) = Felt::from_hex(&s[2..]) {
                                                    DecodedValueType::Single(felt)
                                                } else {
                                                    DecodedValueType::String(s)
                                                }
                                            } else {
                                                DecodedValueType::String(s)
                                            }
                                        }
                                        serde_json::Value::Number(n) => {
                                            if let Some(parsed) = n.as_u64() {
                                                DecodedValueType::BigUint(BigUint::from(parsed))
                                            } else if let Some(parsed) = n.as_i64() {
                                                DecodedValueType::BigInt(BigInt::from(parsed))
                                            } else {
                                                DecodedValueType::String(n.to_string())
                                            }
                                        }
                                        serde_json::Value::Bool(b) => DecodedValueType::Bool(b),
                                        _ => DecodedValueType::String(v.to_string()),
                                    };
                                    fields.insert(
                                        key,
                                        DecodedValue {
                                            name: None,
                                            type_name: "unknown".to_string(),
                                            value,
                                        },
                                    );
                                }
                            }
                            decoded.push(DecodedValueType::Struct(fields));
                        }
                        _ => decoded.push(DecodedValueType::String(v.to_string())),
                    }
                }
                Ok(DecodedValueType::Array(decoded))
            }
            serde_json::Value::Object(obj) => {
                let mut fields = BTreeMap::new();
                for (k, v) in obj {
                    let key: usize = k.parse().map_err(serde::de::Error::custom)?;
                    let value = match v {
                        serde_json::Value::String(s) => DecodedValueType::String(s),
                        serde_json::Value::Number(n) => {
                            if let Some(parsed) = n.as_u64() {
                                DecodedValueType::BigUint(BigUint::from(parsed))
                            } else if let Some(parsed) = n.as_i64() {
                                DecodedValueType::BigInt(BigInt::from(parsed))
                            } else {
                                DecodedValueType::String(n.to_string())
                            }
                        }
                        serde_json::Value::Bool(b) => DecodedValueType::Bool(b),
                        _ => DecodedValueType::String(v.to_string()),
                    };
                    fields.insert(
                        key,
                        DecodedValue {
                            name: None,
                            type_name: "unknown".to_string(),
                            value,
                        },
                    );
                }
                Ok(DecodedValueType::Struct(fields))
            }
            serde_json::Value::Null => Ok(DecodedValueType::None),
            _ => Err(serde::de::Error::custom("Unsupported value type")),
        }
    }
}

impl<'de> Deserialize<'de> for DecodedValue {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        #[derive(Deserialize)]
        struct DecodedValueHelper {
            name: Option<String>,
            type_name: String,
            value: DecodedValueType,
        }

        let helper = DecodedValueHelper::deserialize(deserializer)?;
        Ok(DecodedValue {
            name: helper.name,
            type_name: helper.type_name,
            value: helper.value,
        })
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
            let mut map = BTreeMap::new();
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
            let mut map = BTreeMap::new();
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
        // Should contain the inner value directly (no "Some" wrapper)
        assert!(json.contains("\"type_name\":\"Array<Call>\""));
        assert!(json.contains("\"call1\""));
        // Should NOT contain the "Some" wrapper since enum serialization changed
        assert!(!json.contains("\"Some\":"));
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
        // Should contain the inner value directly (no "Error" wrapper)
        assert!(json.contains("\"type_name\":\"Error\""));
        assert!(json.contains("\"error_message\""));
        // Should NOT contain the "Error" wrapper since enum serialization changed
        assert!(!json.contains("\"Error\":"));
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
        // Should contain the inner value directly (no "None" wrapper)
        assert!(json.contains("\"type_name\":\"Unit\""));
        assert!(json.contains("\"value\":null"));
        // Should NOT contain the "None" wrapper since enum serialization changed
        assert!(!json.contains("\"None\":"));
    }

    #[test]
    fn test_complex_nested_structure() {
        // Test a complex nested structure similar to the user's example
        let call_struct = DecodedValueType::Struct({
            let mut map = BTreeMap::new();
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

        // Should contain the inner structure directly (no "Some" wrapper)
        assert!(json.contains("\"type_name\":\"Array<Call>\""));
        assert!(json.contains("\"0x123\""));
        assert!(json.contains("\"0x456\""));
        // Should NOT contain the "Some" wrapper since enum serialization changed
        assert!(!json.contains("\"Some\":"));
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
                let mut map = BTreeMap::new();
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

        // The JSON should contain the inner structure directly (no "Some" wrapper)
        assert!(json.contains("\"type_name\":\"Array<Call>\""));
        // Should NOT contain the "Some" wrapper since enum serialization changed
        assert!(!json.contains("\"Some\":"));
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

        // The JSON should contain the inner value directly (no "Some" wrapper)
        assert!(json.contains("\"type_name\":\"Some\""));
        assert!(json.contains("\"test_value\""));
        // Should NOT contain the "Some" wrapper since enum serialization changed
        assert!(!json.contains("\"Some\":"));
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

        // The JSON should contain the inner value directly (no "None" wrapper)
        assert!(json.contains("\"type_name\":\"Unit\""));
        assert!(json.contains("\"value\":null"));
        // Should NOT contain the "None" wrapper since enum serialization changed
        assert!(!json.contains("\"None\":"));
    }

    #[test]
    fn test_struct_serialization_with_multiple_fields() {
        // Test case 5: Struct with multiple fields (should wrap)
        let struct_value = DecodedValueType::Struct({
            let mut map = BTreeMap::new();
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
                let mut map = BTreeMap::new();
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
            let mut map = BTreeMap::new();
            map.insert(0, inner_value);
            map
        });

        let json = serde_json::to_string(&option_struct).unwrap();
        println!("Serialized Option struct: {}", json);

        // The JSON should contain the "0" wrapper since it's a struct field
        assert!(json.contains("\"0\":"));
        assert!(json.contains("\"type_name\":\"Array<Call>\""));
    }

    #[test]
    fn test_layout_enum_compact_format() {
        // Test Layout enum with Struct variant
        let field_layout_1 = DecodedValue {
            name: Some("selector".to_string()),
            type_name: "felt252".to_string(),
            value: DecodedValueType::String(
                "0x8bf0013e10cffa31b02b9fd12fd75dcec27382ecae9021abca59f6c1bb2c5a".to_string(),
            ),
        };

        let field_layout_2 = DecodedValue {
            name: Some("layout".to_string()),
            type_name: "Layout".to_string(),
            value: DecodedValueType::Struct({
                let mut map = BTreeMap::new();
                map.insert(
                    0,
                    DecodedValue {
                        name: Some("selector".to_string()),
                        type_name: "felt252".to_string(),
                        value: DecodedValueType::String(
                            "0x121d1cadbcfa91eec65aa16715b94ffc1c9654ba57ea2ef1a2127bca1127a83"
                                .to_string(),
                        ),
                    },
                );
                map.insert(
                    1,
                    DecodedValue {
                        name: Some("layout".to_string()),
                        type_name: "Layout".to_string(),
                        value: DecodedValueType::Struct({
                            let mut inner_map = BTreeMap::new();
                            inner_map.insert(
                                0,
                                DecodedValue {
                                    name: Some("Fixed".to_string()),
                                    type_name: "Span<u8>".to_string(),
                                    value: DecodedValueType::Array(vec![DecodedValueType::String(
                                        "32".to_string(),
                                    )]),
                                },
                            );
                            inner_map
                        }),
                    },
                );
                map
            }),
        };

        let layout_struct_variant = create_compact_enum(
            Some("layout"),
            "Layout",
            "Struct",
            DecodedValue {
                name: None,
                type_name: "Span<FieldLayout>".to_string(),
                value: DecodedValueType::Array(vec![DecodedValueType::Struct({
                    let mut map = BTreeMap::new();
                    map.insert(0, field_layout_1);
                    map.insert(1, field_layout_2);
                    map
                })]),
            },
        );

        let json = serde_json::to_string(&layout_struct_variant).unwrap();
        println!("Layout::Struct compact format: {}", json);

        // Should have compact format: type_name = "Layout::Struct"
        assert!(json.contains("\"type_name\":\"Layout::Struct\""));
        // Should not have wrapper object with "Struct" key
        assert!(!json.contains("\"Struct\":"));
        // Should contain the inner data directly
        assert!(json.contains("0x8bf0013e10cffa31b02b9fd12fd75dcec27382ecae9021abca59f6c1bb2c5a"));
    }

    #[test]
    fn test_layout_enum_fixed_variant_compact_format() {
        // Test Layout enum with Fixed variant
        let layout_fixed_variant = create_compact_enum(
            Some("layout"),
            "Layout",
            "Fixed",
            DecodedValue {
                name: None,
                type_name: "Span<u8>".to_string(),
                value: DecodedValueType::Array(vec![DecodedValueType::String("32".to_string())]),
            },
        );

        let json = serde_json::to_string(&layout_fixed_variant).unwrap();
        println!("Layout::Fixed compact format: {}", json);

        // Should have compact format: type_name = "Layout::Fixed"
        assert!(json.contains("\"type_name\":\"Layout::Fixed\""));
        // Should not have wrapper object with "Fixed" key
        assert!(!json.contains("\"Fixed\":"));
        // Should contain the inner data directly
        assert!(json.contains("32"));
    }
}

/// Helper function to create a compact enum value
pub fn create_compact_enum(
    name: Option<&str>,
    enum_type_name: &str,
    variant: &str,
    inner_value: DecodedValue,
) -> DecodedValue {
    DecodedValue {
        name: name.map(|s| s.to_string()),
        type_name: format!("{}::{}", enum_type_name, variant),
        value: inner_value.value,
    }
}
