use crate::common::create_result_obj;
use crate::{simplify_type_name, skip_builtin_type_declaration};
use cairo_lang_sierra::ids::{ConcreteTypeId, GenericTypeId};
use cairo_lang_sierra::program::{GenericArg, TypeDeclaration};
use fancy_regex::Regex;
use serde_json::json;
use serde_json::Value;
use starknet_types_core::felt::Felt as Felt252;
use tracing::warn;

pub fn internal_decode_datas(
    values: &mut Vec<String>,
    type_id: &ConcreteTypeId,
    type_declarations: &[TypeDeclaration],
    relocated_memory: &Vec<Option<Felt252>>,
    data_index: &mut usize,
) -> Vec<Value> {
    let mut result = Vec::new();

    if let Some(type_declaration) = type_declarations
        .iter()
        .find(|decl| decl.id == *type_id)
        .cloned()
    {
        let debug_name =
            simplify_type_name(type_declaration.id.debug_name.as_deref().unwrap_or(""));
        let generic_type_id = &type_declaration.long_id.generic_id;

        if !skip_builtin_type_declaration(debug_name.as_str()) {
            let enum_type = GenericTypeId::from_string("Enum");
            let struct_type = GenericTypeId::from_string("Struct");
            let array_type = GenericTypeId::from_string("Array");
            let snapshot_type = GenericTypeId::from_string("Snapshot");

            if *generic_type_id == enum_type {
                //NOTE: We need to handle bool as special case, as it is Enum, and than it is
                //desctructed as the Struct of two Unit value
                if debug_name == "bool" {
                    *data_index += 1;
                    let bool_value = values
                        .get(*data_index)
                        .and_then(|val| val.parse::<u8>().ok());

                    if let Some(value) = bool_value {
                        let bool_result = create_result_obj(&[], *data_index, "bool", json!(value));
                        result.push(json!(bool_result));
                    }
                } else if let Some(value) = values.get(*data_index) {
                    if let Ok(index) = value.parse::<usize>() {
                        if index + 1 < type_declaration.long_id.generic_args.len() {
                            if let Some(arg) = type_declaration.long_id.generic_args.get(index + 1)
                            {
                                if let GenericArg::Type(concrete_type_id) = arg {
                                    *data_index += 1;
                                    let decoded_enum_value = internal_decode_datas(
                                        values,
                                        concrete_type_id,
                                        type_declarations,
                                        relocated_memory,
                                        data_index,
                                    );

                                    result.extend(decoded_enum_value);
                                }
                            }
                        }
                    }
                }
            } else if *generic_type_id == struct_type {
                let mut decoded_struct_values = serde_json::Map::new();
                for (i, arg) in type_declaration.long_id.generic_args.iter().enumerate() {
                    if let GenericArg::Type(concrete_type_id) = arg {
                        let decoded_value = internal_decode_datas(
                            values,
                            concrete_type_id,
                            type_declarations,
                            relocated_memory,
                            data_index,
                        );

                        for decoded_field in decoded_value {
                            decoded_struct_values.insert(i.to_string(), decoded_field.clone());
                        }
                    }
                }

                if !decoded_struct_values.is_empty() {
                    let decoded = create_result_obj(
                        &[],
                        *data_index,
                        &debug_name,
                        Value::Object(decoded_struct_values),
                    );
                    result.push(Value::Object(decoded));
                }
            } else if *generic_type_id == array_type {
                let mut decoded_array_values = Vec::new();
                let mut array_length = 0;
                if !relocated_memory.is_empty() && *data_index + 1 < values.len() {
                    let mut extracted_values = values[..*data_index].to_vec();

                    let (size, memory_values) =
                        extract_memory_values(relocated_memory, values, data_index);
                    array_length = size;
                    extracted_values.extend(memory_values);
                    extracted_values.extend(values[*data_index + 2..].iter().cloned());

                    values.clear();
                    values.extend(extracted_values);
                }

                for _ in 0..array_length {
                    for arg in &type_declaration.long_id.generic_args {
                        if let GenericArg::Type(concrete_type_id) = arg {
                            let decoded_element = internal_decode_datas(
                                values,
                                concrete_type_id,
                                type_declarations,
                                relocated_memory,
                                data_index,
                            );

                            for element in decoded_element {
                                if let Some(obj) = element.as_object() {
                                    if let Some(inner_value) = obj.get("value") {
                                        decoded_array_values.push(inner_value.clone());
                                    }
                                }
                            }
                        }
                    }
                }

                if !decoded_array_values.is_empty() {
                    let decoded = create_result_obj(
                        &[],
                        *data_index,
                        &debug_name,
                        Value::Array(decoded_array_values),
                    );
                    result.push(Value::Object(decoded));
                }
            } else if *generic_type_id == snapshot_type {
                let mut decoded_snapshot_values = Vec::new();
                for arg in &type_declaration.long_id.generic_args {
                    if let GenericArg::Type(concrete_type_id) = arg {
                        let decoded_snapshot = internal_decode_datas(
                            values,
                            concrete_type_id,
                            type_declarations,
                            relocated_memory,
                            data_index,
                        );
                        for element in decoded_snapshot {
                            if let Some(obj) = element.as_object() {
                                if let Some(inner_value) = obj.get("value") {
                                    decoded_snapshot_values.push(inner_value.clone());
                                }
                            }
                        }

                        result.extend(decoded_snapshot_values.clone());
                    }
                }
            } else {
                if type_declaration.long_id.generic_args.is_empty() {
                    if let Some(value) = values.get(*data_index) {
                        result.push(Value::Object(create_result_obj(
                            &[],
                            0,
                            debug_name.as_str(),
                            Value::String(value.to_string()),
                        )));
                    }
                    *data_index += 1;
                }
            }
        }
    }

    result
}

fn extract_memory_values(
    relocated_memory: &Vec<Option<Felt252>>,
    values: &[String],
    value_index: &usize,
) -> (usize, Vec<String>) {
    let mut size = 0;
    let mut memory_values: Vec<String> = Vec::new();

    let start_index = match values
        .get(*value_index)
        .and_then(|v| v.parse::<usize>().ok())
    {
        Some(index) => index,
        None => {
            warn!(
                "Failed to parse start index from values at position {}",
                value_index
            );
            return (size, memory_values);
        }
    };

    let end_index = match values
        .get(*value_index + 1)
        .and_then(|v| v.parse::<usize>().ok())
    {
        Some(index) => index,
        None => {
            warn!(
                "Failed to parse end index from values at position {}",
                value_index + 1
            );
            return (size, memory_values);
        }
    };
    if end_index < start_index {
        return (size, memory_values);
    }

    if start_index == end_index {
        size = 1;
        if let Some(Some(inner_value)) = relocated_memory.get(start_index) {
            memory_values.push(inner_value.to_string());
        }
    } else {
        size = end_index - start_index;

        for index in start_index..end_index {
            if let Some(Some(inner_value)) = relocated_memory.get(index) {
                memory_values.push(inner_value.to_string());
            }
        }
    }
    (size, memory_values)
}

pub fn clean_return_tuple_type(simplified_type_name: &str) -> String {
    // Regex for matching and removing Option::<...>
    let re_option = Regex::new(r"Option::<([^>]+)>").unwrap();

    // Step 1: Remove `Option::<...>` and extract inner content
    let cleaned_option_type = re_option
        .replace_all(simplified_type_name, "$1")
        .to_string();

    cleaned_option_type
}
