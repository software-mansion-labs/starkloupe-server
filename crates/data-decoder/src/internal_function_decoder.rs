use std::collections::HashMap;

use crate::common::create_result_obj;
use crate::{simplify_type_name, skip_builtin_type_declaration};
use cairo_lang_sierra::ids::{ConcreteTypeId, GenericTypeId};
use cairo_lang_sierra::program::{GenericArg, TypeDeclaration};
use serde_json::json;
use serde_json::Value;
use starknet_types_core::felt::Felt as Felt252;

pub fn internal_decode_datas(
    values: &mut Vec<String>,
    type_id: &ConcreteTypeId,
    type_declaration_map: &HashMap<ConcreteTypeId, TypeDeclaration>,
    relocated_memory: &[Option<Felt252>],
    data_index: &mut usize,
) -> Vec<Value> {
    let mut result = Vec::new();

    if let Some(type_declaration) = type_declaration_map.get(type_id) {
        let debug_name =
            simplify_type_name(type_declaration.id.debug_name.as_deref().unwrap_or(""));
        let generic_type_id = &type_declaration.long_id.generic_id;

        if !skip_builtin_type_declaration(debug_name.as_str()) {
            if type_declaration.long_id.generic_args.is_empty() {
                if let Some(value) = values.get(*data_index) {
                    result.push(Value::Object(create_result_obj(
                        &[],
                        *data_index,
                        debug_name.as_str(),
                        Value::String(value.to_string()),
                    )));
                    *data_index += 1;
                }
                return result;
            }
            if *generic_type_id == GenericTypeId::from_string("Enum") {
                if debug_name == "bool" {
                    handle_bool_case(values, data_index, &mut result);
                } else {
                    handle_enum_case(
                        values,
                        type_declaration,
                        data_index,
                        relocated_memory,
                        type_declaration_map,
                        &mut result,
                    );
                }
            } else if *generic_type_id == GenericTypeId::from_string("Struct") {
                let mut decoded_struct_values = serde_json::Map::new();
                for (i, arg) in type_declaration.long_id.generic_args.iter().enumerate() {
                    if let GenericArg::Type(concrete_type_id) = arg {
                        let decoded_value = internal_decode_datas(
                            values,
                            concrete_type_id,
                            type_declaration_map,
                            relocated_memory,
                            data_index,
                        );
                        for decoded_field in decoded_value {
                            decoded_struct_values.insert(i.to_string(), decoded_field.clone());
                        }
                    }
                }
                if !decoded_struct_values.is_empty() {
                    result.push(Value::Object(create_result_obj(
                        &[],
                        *data_index,
                        &debug_name,
                        Value::Object(decoded_struct_values),
                    )));
                }
            } else if *generic_type_id == GenericTypeId::from_string("Array") {
                let mut array_length = 0;
                if !relocated_memory.is_empty() && *data_index + 1 < values.len() {
                    let (size, memory_values) =
                        extract_memory_values(relocated_memory, values, data_index);
                    array_length = size;
                    values.splice(*data_index..*data_index + 2, memory_values);
                }

                let decoded_array_values = decode_array_elements(
                    type_declaration,
                    array_length,
                    values,
                    relocated_memory,
                    data_index,
                    type_declaration_map,
                );
                if !decoded_array_values.is_empty() {
                    result.push(Value::Object(create_result_obj(
                        &[],
                        *data_index,
                        &debug_name,
                        Value::Array(decoded_array_values),
                    )));
                }
            } else if *generic_type_id == GenericTypeId::from_string("Snapshot") {
                let decoded_snapshot_values = decode_snapshot_elements(
                    type_declaration,
                    values,
                    data_index,
                    relocated_memory,
                    type_declaration_map,
                );
                result.extend(decoded_snapshot_values);
            }
        }
    }

    result
}

fn handle_bool_case(values: &mut [String], data_index: &mut usize, result: &mut Vec<Value>) {
    *data_index += 1;
    if let Some(bool_value) = values
        .get(*data_index)
        .and_then(|val| val.parse::<u8>().ok())
    {
        let bool_result = create_result_obj(&[], *data_index, "bool", json!(bool_value));
        result.push(json!(bool_result));
    }
}

fn handle_enum_case(
    values: &mut Vec<String>,
    type_declaration: &TypeDeclaration,
    data_index: &mut usize,
    relocated_memory: &[Option<Felt252>],
    type_declaration_map: &HashMap<ConcreteTypeId, TypeDeclaration>,
    result: &mut Vec<Value>,
) {
    if let Some(value) = values.get(*data_index) {
        if let Ok(index) = value.parse::<usize>() {
            if let Some(GenericArg::Type(concrete_type_id)) =
                type_declaration.long_id.generic_args.get(index + 1)
            {
                *data_index += 1;
                let decoded_enum_value = internal_decode_datas(
                    values,
                    concrete_type_id,
                    type_declaration_map,
                    relocated_memory,
                    data_index,
                );
                result.extend(decoded_enum_value);
            }
        }
    }
}

fn decode_array_elements(
    type_declaration: &TypeDeclaration,
    array_length: usize,
    values: &mut Vec<String>,
    relocated_memory: &[Option<Felt252>],
    data_index: &mut usize,
    type_declaration_map: &HashMap<ConcreteTypeId, TypeDeclaration>,
) -> Vec<Value> {
    let mut decoded_array_values = Vec::new();
    for _ in 0..array_length {
        for arg in &type_declaration.long_id.generic_args {
            if let GenericArg::Type(concrete_type_id) = arg {
                let decoded_element = internal_decode_datas(
                    values,
                    concrete_type_id,
                    type_declaration_map,
                    relocated_memory,
                    data_index,
                );
                decoded_array_values.extend(
                    decoded_element
                        .iter()
                        .filter_map(|e| e.get("value").cloned())
                        .collect::<Vec<Value>>(),
                );
            }
        }
    }
    decoded_array_values
}

fn decode_snapshot_elements(
    type_declaration: &TypeDeclaration,
    values: &mut Vec<String>,
    data_index: &mut usize,
    relocated_memory: &[Option<Felt252>],
    type_declaration_map: &HashMap<ConcreteTypeId, TypeDeclaration>,
) -> Vec<Value> {
    let mut decoded_snapshot_values = Vec::new();

    for arg in &type_declaration.long_id.generic_args {
        if let GenericArg::Type(concrete_type_id) = arg {
            let decoded_snapshot = internal_decode_datas(
                values,
                concrete_type_id,
                type_declaration_map,
                relocated_memory,
                data_index,
            );

            for element in decoded_snapshot {
                if let Some(obj) = element.as_object() {
                    // Try to retrieve the 'value' field from the decoded element
                    if let Some(inner_value) = obj.get("value") {
                        decoded_snapshot_values.push(inner_value.clone());
                    }
                }
            }
        }
    }

    decoded_snapshot_values
}

fn extract_memory_values(
    relocated_memory: &[Option<Felt252>],
    values: &[String],
    value_index: &usize,
) -> (usize, Vec<String>) {
    if let (Some(start_index), Some(end_index)) = (
        values
            .get(*value_index)
            .and_then(|v| v.parse::<usize>().ok()),
        values
            .get(*value_index + 1)
            .and_then(|v| v.parse::<usize>().ok()),
    ) {
        if end_index >= start_index {
            let size = end_index - start_index + 1;
            let memory_values: Vec<String> = (start_index..=end_index)
                .filter_map(|index| {
                    relocated_memory
                        .get(index)
                        .and_then(|v| (*v).map(|inner| inner.to_string()))
                })
                .collect();

            return (size, memory_values);
        }
    }
    (0, Vec::new())
}
