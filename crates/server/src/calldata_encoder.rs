use crate::abi_fetcher::fetch_contract_abi;
use data_decoder::{split_into_limbs, DecodedValue};
use std::collections::HashMap;
use std::sync::Arc;
use walnut_shared::abi::{Enum, Struct};
use walnut_shared::utils::simplify_type_name;

/// Parse tuple type string into individual element types
/// Handles nested tuples, arrays, and generic types properly
#[inline]
fn parse_tuple_types(tuple_inner: &str) -> Vec<&str> {
    let mut types = Vec::new();
    let mut start = 0;
    let mut depth_paren = 0;
    let mut depth_angle = 0;

    for (i, ch) in tuple_inner.char_indices() {
        match ch {
            '(' => depth_paren += 1,
            ')' => depth_paren -= 1,
            '<' => depth_angle += 1,
            '>' => depth_angle -= 1,
            ',' if depth_paren == 0 && depth_angle == 0 => {
                let segment = tuple_inner[start..i].trim();
                if !segment.is_empty() {
                    types.push(segment);
                }
                start = i + 1;
            }
            _ => {}
        }
    }

    let segment = tuple_inner[start..].trim();
    if !segment.is_empty() {
        types.push(segment);
    }

    types
}

/// Extract inner type from generic type (e.g., "Array<T>" -> "T")
#[inline]
fn extract_generic_inner(type_name: &str) -> Option<&str> {
    let start = type_name.find('<')? + 1;
    let end = type_name.rfind('>')?;
    if start < end {
        Some(&type_name[start..end])
    } else {
        None
    }
}

/// Encode a single decoded value to calldata format
pub fn encode_decoded_value(
    value: &DecodedValue,
    enums: &HashMap<String, Enum>,
    structs: &HashMap<String, Struct>,
) -> Result<Vec<String>, String> {
    match &value.value {
        data_decoder::DecodedValueType::String(s) => {
            // Check if this is an enum variant (type_name contains "::" or value is not a hex string)
            if value.type_name.contains("::")
                || (!s.starts_with("0x") && !s.chars().all(|c| c.is_ascii_hexdigit()))
            {
                // This is likely an enum variant
                let enum_name = value
                    .type_name
                    .split("::")
                    .next()
                    .unwrap_or(&value.type_name);

                // Find enum definition
                if let Some(enum_def) = enums.get(enum_name) {
                    // Find variant index by name
                    let variant_index = enum_def
                        .variants
                        .iter()
                        .position(|v| v.name == *s)
                        .ok_or_else(|| {
                            format!(
                                "Variant '{}' not found in enum '{}'. Available variants: {:?}",
                                s,
                                enum_name,
                                enum_def
                                    .variants
                                    .iter()
                                    .map(|v| &v.name)
                                    .collect::<Vec<_>>()
                            )
                        })?;

                    Ok(vec![format!("0x{:x}", variant_index)])
                } else {
                    // Enum definition not found - return string as-is
                    Ok(vec![s.clone()])
                }
            } else {
                // Regular string or hex value
                Ok(vec![s.clone()])
            }
        }
        data_decoder::DecodedValueType::Single(felt) => Ok(vec![felt.to_hex_string()]),
        data_decoder::DecodedValueType::BigUint(n) => {
            // Special handling for multi-limb integer types (u256, u512)
            // These types are decoded as single BigUint values for readability,
            // but must be split back into limbs for transaction encoding
            Ok(split_into_limbs(n, &value.type_name))
        }
        data_decoder::DecodedValueType::BigInt(n) => Ok(vec![format!("0x{:x}", n)]),
        data_decoder::DecodedValueType::Bool(b) => Ok(vec![if *b {
            "0x1".to_string()
        } else {
            "0x0".to_string()
        }]),
        data_decoder::DecodedValueType::Array(items) => {
            let type_name = &value.type_name;
            let is_tuple = type_name.starts_with('(') || type_name.starts_with("Tuple<");
            let mut calldata = Vec::new();

            if is_tuple {
                // Extract tuple element types
                let inner = if type_name.starts_with("Tuple<") {
                    type_name
                        .strip_prefix("Tuple<")
                        .and_then(|s| s.strip_suffix('>'))
                        .unwrap_or("")
                } else {
                    type_name
                        .strip_prefix('(')
                        .and_then(|s| s.strip_suffix(')'))
                        .unwrap_or("")
                };

                let element_types = parse_tuple_types(inner);

                // Encode each tuple element with its correct type
                for (i, item) in items.iter().enumerate() {
                    let elem_type = element_types.get(i).copied().unwrap_or("unknown");
                    let item_value = DecodedValue {
                        name: None,
                        type_name: elem_type.to_string(),
                        value: item.clone(),
                    };
                    calldata.extend(encode_decoded_value(&item_value, enums, structs)?);
                }
            } else {
                // Array - add length prefix
                calldata.push(format!("0x{:x}", items.len()));

                // Extract element type from Array<T> or Span<T>
                let element_type =
                    if type_name.starts_with("Array<") || type_name.starts_with("Span<") {
                        extract_generic_inner(type_name).unwrap_or("unknown")
                    } else {
                        "unknown"
                    };

                // Encode array elements
                for item in items {
                    let item_value = DecodedValue {
                        name: None,
                        type_name: element_type.to_string(),
                        value: item.clone(),
                    };
                    calldata.extend(encode_decoded_value(&item_value, enums, structs)?);
                }
            }

            Ok(calldata)
        }
        data_decoder::DecodedValueType::Struct(fields) => {
            let mut calldata = Vec::with_capacity(fields.len() * 2);
            for param in fields.values() {
                calldata.extend(encode_decoded_value(param, enums, structs)?);
            }
            Ok(calldata)
        }
        data_decoder::DecodedValueType::Enum(variant, inner) => {
            // Parse enum name from type_name (before "::")
            let enum_name = value.type_name.split("::").next().unwrap_or("unknown");

            // Find enum definition
            let enum_def = enums
                .get(enum_name)
                .ok_or_else(|| format!("Enum '{}' not found in ABI", enum_name))?;

            // Find variant index
            let variant_index = enum_def
                .variants
                .iter()
                .position(|v| v.name == *variant)
                .ok_or_else(|| {
                    format!("Variant '{}' not found in enum '{}'", variant, enum_name)
                })?;

            let mut calldata = Vec::with_capacity(2);
            calldata.push(format!("0x{:x}", variant_index));

            // Encode inner value
            let inner_value = DecodedValue {
                name: None,
                type_name: inner.type_name.clone(),
                value: inner.value.clone(),
            };
            calldata.extend(encode_decoded_value(&inner_value, enums, structs)?);

            Ok(calldata)
        }
        data_decoder::DecodedValueType::None => Ok(vec![]),
    }
}

/// Check if type_name represents an enum variant (format: "EnumName::VariantName")
pub fn is_enum_variant(type_name: &str) -> bool {
    type_name.contains("::") && type_name.split("::").count() == 2
}

/// Encode enum variant from type_name format
pub fn encode_enum_variant(
    value: &DecodedValue,
    enums: &HashMap<String, Enum>,
) -> Result<Vec<String>, String> {
    let parts: Vec<&str> = value.type_name.split("::").collect();
    if parts.len() != 2 {
        return Err("Invalid enum variant format".to_string());
    }

    let enum_name = parts[0];
    let variant_name = parts[1];

    // Find enum definition
    let enum_def = enums
        .get(enum_name)
        .ok_or_else(|| format!("Enum '{}' not found in ABI", enum_name))?;

    // Find variant index
    let variant_index = enum_def
        .variants
        .iter()
        .position(|v| v.name == variant_name)
        .ok_or_else(|| {
            format!(
                "Variant '{}' not found in enum '{}'",
                variant_name, enum_name
            )
        })?;

    let mut calldata = Vec::new();

    // Add variant index
    calldata.push(format!("0x{:x}", variant_index));

    // Encode the value based on variant type
    match &value.value {
        data_decoder::DecodedValueType::String(_s) => {
            // For unit variants, the string value is just the variant name
            // We don't need to add it as calldata since we already have the variant index
            // This is handled by the variant index above
        }
        data_decoder::DecodedValueType::Single(felt) => {
            calldata.push(felt.to_hex_string());
        }
        data_decoder::DecodedValueType::BigUint(n) => {
            // Special handling for multi-limb integer types (u256, u512)
            // Note: For enum variants, we use the variant's type, not the enum's type
            let variant_type = value.type_name.rsplit("::").next().unwrap_or("");
            let limbs = split_into_limbs(n, variant_type);
            calldata.extend(limbs);
        }
        data_decoder::DecodedValueType::BigInt(n) => {
            calldata.push(format!("0x{:x}", n));
        }
        data_decoder::DecodedValueType::Bool(b) => {
            calldata.push(if *b {
                "0x1".to_string()
            } else {
                "0x0".to_string()
            });
        }
        data_decoder::DecodedValueType::Array(items) => {
            for item in items {
                let item_value = DecodedValue {
                    name: None,
                    type_name: "unknown".to_string(),
                    value: item.clone(),
                };
                let encoded_item = encode_decoded_value(&item_value, enums, &HashMap::new())?;
                calldata.extend(encoded_item);
            }
        }
        data_decoder::DecodedValueType::Struct(fields) => {
            for param in fields.values() {
                let encoded_field = encode_decoded_value(param, enums, &HashMap::new())?;
                calldata.extend(encoded_field);
            }
        }
        data_decoder::DecodedValueType::Enum(_inner_variant, inner_value) => {
            let inner_encoded = encode_decoded_value(inner_value, enums, &HashMap::new())?;
            calldata.extend(inner_encoded);
        }
        data_decoder::DecodedValueType::None => {
            // For unit types (empty variants), no additional data needed
        }
    }

    Ok(calldata)
}

/// Encode parameters using ABI information
pub fn encode_parameters_with_abi(
    parameters: &[DecodedValue],
    function_inputs: &[walnut_shared::abi::Input],
    structs: &HashMap<String, Struct>,
    enums: &HashMap<String, Enum>,
) -> Result<Vec<String>, String> {
    let mut calldata = Vec::with_capacity(parameters.len() * 2);

    for (i, param) in parameters.iter().enumerate() {
        // Check if this is an enum variant (type_name format: "EnumName::VariantName")
        if is_enum_variant(&param.type_name) {
            calldata.extend(encode_enum_variant(param, enums)?);
            continue;
        }

        // For regular parameters, use ABI type if available, otherwise use parameter's own type
        let input_type = function_inputs
            .get(i)
            .map(|input| input.ty.as_str())
            .unwrap_or(&param.type_name);

        let simplified_type = simplify_type_name(input_type);

        // Create a new DecodedValue with the correct type_name for encoding
        let param_with_type = DecodedValue {
            name: param.name.clone(),
            type_name: simplified_type,
            value: param.value.clone(),
        };

        calldata.extend(encode_decoded_value(&param_with_type, enums, structs)?);
    }

    Ok(calldata)
}

/// Basic parameter encoding without ABI (fallback)
pub fn encode_parameters_basic(
    parameters: &HashMap<usize, DecodedValue>,
    enums: &HashMap<String, Enum>,
    structs: &HashMap<String, Struct>,
) -> Result<Vec<String>, String> {
    let mut calldata = Vec::new();

    for param in parameters.values() {
        let encoded = encode_decoded_value(param, enums, structs)?;
        calldata.extend(encoded);
    }

    Ok(calldata)
}

/// Encode decoded calldata back to raw calldata format with ABI information.
/// ABI fetches for all contract calls are performed in parallel.
pub async fn encode_decoded_calldata(
    decoded_calls: &[crate::handlers::simulate::ContractCall],
    chain_id: &str,
) -> Result<Vec<String>, String> {
    use crate::abi_fetcher::ContractAbiData;

    // Fetch all ABIs in parallel
    let abi_futures: Vec<_> = decoded_calls
        .iter()
        .map(|call| fetch_contract_abi(&call.contract_address, chain_id))
        .collect();
    let abi_results: Vec<Result<Arc<ContractAbiData>, String>> =
        futures::future::join_all(abi_futures).await;

    let mut raw_calldata = Vec::new();
    // Add number of calls
    raw_calldata.push(format!("0x{:x}", decoded_calls.len()));

    for (call, abi_result) in decoded_calls.iter().zip(abi_results) {
        raw_calldata.push(call.contract_address.clone());
        raw_calldata.push(call.function_selector.clone());

        let call_calldata = match abi_result {
            Ok(abi_data) => {
                // Find the function by name
                let function = abi_data
                    .functions
                    .iter()
                    .find(|f| f.name == call.function_name.as_deref().unwrap_or(""))
                    .ok_or_else(|| {
                        format!(
                            "Function with name {} not found in ABI",
                            call.function_name.as_deref().unwrap_or("unknown")
                        )
                    })?;

                // Encode parameters using ABI
                encode_parameters_with_abi(
                    &call.parameters,
                    function.inputs.as_slice(),
                    &abi_data.structs,
                    &abi_data.enums,
                )?
            }
            Err(_e) => {
                // Fallback to basic encoding without ABI
                let params_map: HashMap<usize, DecodedValue> = call
                    .parameters
                    .iter()
                    .enumerate()
                    .map(|(i, v)| (i, v.clone()))
                    .collect();

                encode_parameters_basic(&params_map, &HashMap::new(), &HashMap::new())?
            }
        };

        raw_calldata.push(format!("0x{:x}", call_calldata.len()));
        raw_calldata.extend(call_calldata);
    }

    Ok(raw_calldata)
}
