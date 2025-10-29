use crate::abi_fetcher::fetch_contract_abi;
use data_decoder::{split_into_limbs, DecodedValue};
use std::collections::HashMap;
use walnut_shared::abi::{Enum, Struct};
use walnut_shared::utils::simplify_type_name;

/// Parse tuple type string into individual element types
/// Handles nested tuples, arrays, and generic types properly
fn parse_tuple_types(tuple_inner: &str) -> Vec<String> {
    let mut types = Vec::new();
    let mut current = String::new();
    let mut depth_paren = 0;
    let mut depth_angle = 0;

    for ch in tuple_inner.chars() {
        match ch {
            '(' => {
                depth_paren += 1;
                current.push(ch);
            }
            ')' => {
                depth_paren -= 1;
                current.push(ch);
            }
            '<' => {
                depth_angle += 1;
                current.push(ch);
            }
            '>' => {
                depth_angle -= 1;
                current.push(ch);
            }
            ',' if depth_paren == 0 && depth_angle == 0 => {
                if !current.trim().is_empty() {
                    types.push(current.trim().to_string());
                }
                current.clear();
            }
            _ => current.push(ch),
        }
    }

    if !current.trim().is_empty() {
        types.push(current.trim().to_string());
    }

    types
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
            let mut calldata = Vec::new();

            // Check if this is a tuple (type_name starts with '(' or 'Tuple<')
            let is_tuple =
                value.type_name.starts_with('(') || value.type_name.starts_with("Tuple<");

            // Only add array length for actual arrays, NOT for tuples
            if !is_tuple {
                calldata.push(format!("0x{:x}", items.len()));
            }

            // Try to extract the element type from the array/tuple type_name
            let element_type =
                if value.type_name.starts_with("Array<") || value.type_name.starts_with("Span<") {
                    // Extract the inner type from Array<T> or Span<T>
                    let start = value.type_name.find('<').map(|i| i + 1).unwrap_or(0);
                    let end = value.type_name.rfind('>').unwrap_or(value.type_name.len());
                    if start < end {
                        value.type_name[start..end].to_string()
                    } else {
                        "unknown".to_string()
                    }
                } else if is_tuple {
                    // For tuples, we need to extract individual element types
                    // Parse tuple type like "(Type1, Type2, Type3)" to get element types
                    let inner = if value.type_name.starts_with("Tuple<") {
                        value
                            .type_name
                            .strip_prefix("Tuple<")
                            .and_then(|s| s.strip_suffix('>'))
                            .unwrap_or("")
                    } else {
                        value
                            .type_name
                            .strip_prefix('(')
                            .and_then(|s| s.strip_suffix(')'))
                            .unwrap_or("")
                    };

                    // Split by comma, handling nested types properly
                    let element_types = parse_tuple_types(inner);

                    // Encode each tuple element with its correct type
                    for (i, item) in items.iter().enumerate() {
                        let elem_type = element_types
                            .get(i)
                            .map(|s| s.as_str())
                            .unwrap_or("unknown");

                        let item_value = DecodedValue {
                            name: None,
                            type_name: elem_type.to_string(),
                            value: item.clone(),
                        };
                        let encoded_item = encode_decoded_value(&item_value, enums, structs)?;
                        calldata.extend(encoded_item);
                    }
                    return Ok(calldata);
                } else {
                    "unknown".to_string()
                };

            // For non-tuple arrays
            for (i, item) in items.iter().enumerate() {
                let item_value = DecodedValue {
                    name: None,
                    type_name: element_type.clone(),
                    value: item.clone(),
                };
                let encoded_item = encode_decoded_value(&item_value, enums, structs)?;
                calldata.extend(encoded_item);
            }
            Ok(calldata)
        }
        data_decoder::DecodedValueType::Struct(fields) => {
            let mut calldata = Vec::new();
            for param in fields.values() {
                let encoded_field = encode_decoded_value(param, enums, structs)?;
                calldata.extend(encoded_field);
            }
            Ok(calldata)
        }
        data_decoder::DecodedValueType::Enum(variant, inner) => {
            let mut calldata = Vec::new();

            // Parse enum name from type_name
            let enum_name = value.type_name.split("::").next().unwrap_or("unknown");
            let variant_name = variant;

            // Find enum definition
            if let Some(enum_def) = enums.get(enum_name) {
                // Find variant index
                let variant_index = enum_def
                    .variants
                    .iter()
                    .position(|v| v.name == *variant_name)
                    .ok_or_else(|| {
                        format!(
                            "Variant '{}' not found in enum '{}'",
                            variant_name, enum_name
                        )
                    })?;

                // Add variant index
                calldata.push(format!("0x{:x}", variant_index));

                // Encode inner value
                let inner_value = DecodedValue {
                    name: None,
                    type_name: inner.type_name.clone(),
                    value: inner.value.clone(),
                };
                let encoded_inner = encode_decoded_value(&inner_value, enums, structs)?;
                calldata.extend(encoded_inner);
            } else {
                return Err(format!("Enum '{}' not found in ABI", enum_name));
            }

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
    let mut calldata = Vec::new();

    for (i, param) in parameters.iter().enumerate() {
        // Check if this is an enum variant (type_name format: "EnumName::VariantName")
        if is_enum_variant(&param.type_name) {
            let encoded = encode_enum_variant(param, enums)?;
            calldata.extend(encoded);
            continue;
        }

        // For regular parameters, use ABI type if available, otherwise use parameter's own type
        let input_type = if i < function_inputs.len() {
            &function_inputs[i].ty
        } else {
            &param.type_name
        };

        let simplified_type = simplify_type_name(input_type);

        // Create a new DecodedValue with the correct type_name for encoding
        let param_with_type = DecodedValue {
            name: param.name.clone(),
            type_name: simplified_type,
            value: param.value.clone(),
        };

        let encoded = encode_decoded_value(&param_with_type, enums, structs)?;
        calldata.extend(encoded);
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

/// Encode decoded calldata back to raw calldata format with ABI information
pub async fn encode_decoded_calldata(
    decoded_calls: &[crate::handlers::simulate::ContractCall],
    chain_id: &str,
) -> Result<Vec<String>, String> {
    let mut raw_calldata = Vec::new();

    // Add number of calls
    raw_calldata.push(format!("0x{:x}", decoded_calls.len()));

    for call in decoded_calls {
        // Add contract address
        raw_calldata.push(call.contract_address.clone());
        // Add function selector
        raw_calldata.push(call.function_selector.clone());
        // Fetch ABI for this contract to encode parameters
        match fetch_contract_abi(&call.contract_address, chain_id).await {
            Ok((functions, _type_decoder, structs, enums)) => {
                // Find the function by name
                let function = functions
                    .iter()
                    .find(|f| f.name == call.function_name.as_deref().unwrap_or(""))
                    .ok_or_else(|| {
                        format!(
                            "Function with name {} not found in ABI",
                            call.function_name.as_deref().unwrap_or("unknown")
                        )
                    })?;

                // Encode parameters using ABI
                let call_calldata = encode_parameters_with_abi(
                    &call.parameters,
                    function.inputs.as_slice(),
                    &structs,
                    &enums,
                )?;
                // Add calldata length
                raw_calldata.push(format!("0x{:x}", call_calldata.len()));
                // Add actual calldata
                raw_calldata.extend(call_calldata);
            }
            Err(_e) => {
                // Fallback to basic encoding without ABI
                let call_calldata = encode_parameters_basic(
                    &call
                        .parameters
                        .iter()
                        .enumerate()
                        .map(|(i, v)| (i, v.clone()))
                        .collect(),
                    &HashMap::new(),
                    &HashMap::new(),
                )?;

                // Add calldata length
                raw_calldata.push(format!("0x{:x}", call_calldata.len()));
                // Add actual calldata
                raw_calldata.extend(call_calldata);
            }
        }
    }

    Ok(raw_calldata)
}
