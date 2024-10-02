use crate::common::create_result_obj;
use crate::starknet_types::EDataType;
use crate::{simplify_type_name, skip_builtin_type_declaration};
use cairo_lang_sierra::ids::GenericTypeId;
use cairo_lang_sierra::program::{GenericArg, TypeDeclaration};
use fancy_regex::Regex;
use serde_json::{map::Map, Value};
use starknet_types_core::felt::Felt as Felt252;
use tracing::{info, warn};
use walnut_shared::{Datas, EnumItems, StructItems};

pub fn internal_decode_datas(
    datas: &mut Vec<String>,
    types: &[String],
    names: &[String],
    struct_items: Option<&Vec<StructItems>>,
    enum_items: Option<&Vec<EnumItems>>,
    data_index: &mut usize,
    relocated_memory: &Vec<Option<Felt252>>,
) -> Vec<Value> {
    let mut result = Vec::new();

    for (index, data_type) in types.iter().enumerate() {
        if datas.len() <= *data_index {
            break;
        };
        let e_data_type = EDataType::from_str(data_type, enum_items);
        match e_data_type {
            EDataType::Primitive(_) => {
                let data = match datas.get(*data_index) {
                    Some(value) => value.clone(),
                    None => {
                        warn!("No data found at index {}", *data_index);
                        "Invalid data".to_string()
                    }
                };
                *data_index += 1;
                result.push(Value::Object(create_result_obj(
                    names,
                    index,
                    data_type,
                    Value::String(data),
                )));
            }
            EDataType::Array(inner_type) => {
                let result_value = internal_array(
                    datas,
                    data_index,
                    struct_items,
                    enum_items,
                    &inner_type.to_string(),
                    &relocated_memory,
                    names,
                    index,
                    data_type,
                );
                result.push(result_value)
                // }
            }
            EDataType::UserEnum(_) => {
                let enum_index = match datas.get(*data_index) {
                    Some(value) => value.parse::<usize>().unwrap_or(0),
                    None => {
                        info!(
                            "The first element in the datas is the index of the enum variant {}",
                            *data_index
                        );
                        continue;
                    }
                };
                *data_index += 1;
                if let Some(enum_items) = enum_items {
                    for enum_item in enum_items {
                        if enum_item.name.contains(data_type) {
                            if let Some(enum_member_item) = enum_item.members.get(enum_index) {
                                let variant_name = enum_member_item.names.clone();
                                let enum_type = enum_member_item.types.clone();
                                let decoded_values = internal_decode_datas(
                                    datas,
                                    &vec![enum_type],
                                    &vec![variant_name.clone()],
                                    struct_items,
                                    Some(enum_items),
                                    data_index,
                                    relocated_memory,
                                );

                                result.push(Value::Object(create_result_obj(
                                    names,
                                    index,
                                    data_type,
                                    Value::Array(decoded_values),
                                )));
                            }
                        }
                    }
                }
            }
            EDataType::Tuple(inner_types) => {
                let mut decoded_inner_values = Vec::new();
                if inner_types.len() == 1 && inner_types[0] == "bool" {
                    // If it's a single inner type and it's a bool, we need toget last value from datas
                    if let Some(last_value) = datas.pop() {
                        decoded_inner_values.push(Value::String(last_value));
                    }
                } else {
                    for inner_type in inner_types {
                        if skip_builtin_type_declaration(inner_type.as_str()) {
                            continue;
                        }
                        let inner_type_ref =
                            if inner_type == "Unit" || inner_type.contains("ContractState") {
                                Value::Array(vec![])
                            } else if inner_type.starts_with("Option::<") {
                                // Handle Option::<...> type
                                if let Some(stripped_inner) = inner_type
                                    .strip_prefix("Option::<")
                                    .and_then(|s| s.strip_suffix('>'))
                                {
                                    // Decode the inner value inside Option
                                    let _removed = datas.remove(*data_index); // Remove the current data element that  represent Option
                                    let decoded_inner = internal_decode_datas(
                                        datas,
                                        &[stripped_inner.to_string()],
                                        names,
                                        struct_items,
                                        enum_items,
                                        data_index,
                                        relocated_memory,
                                    );
                                    decoded_inner
                                        .first()
                                        .cloned()
                                        .unwrap_or(Value::String("None".to_string()))
                                } else {
                                    Value::String("None".to_string())
                                }
                            } else {
                                let decoded_inner = internal_decode_datas(
                                    datas,
                                    &[inner_type.to_string()],
                                    names,
                                    struct_items,
                                    enum_items,
                                    data_index,
                                    relocated_memory,
                                );
                                decoded_inner.first().cloned().unwrap_or(Value::Null)
                            };

                        decoded_inner_values.push(inner_type_ref);
                    }
                }
                let decoded_data: Map<String, Value> = decoded_inner_values
                    .into_iter()
                    .enumerate()
                    .map(|(index, item)| (index.to_string(), item))
                    .collect();

                result.push(Value::Object(create_result_obj(
                    names,
                    index,
                    data_type,
                    Value::Object(decoded_data),
                )));
            }
            EDataType::Struct(_) => {
                let decoded_item = decode_internal_struct_item(
                    struct_items,
                    enum_items,
                    datas,
                    data_type,
                    data_index,
                    relocated_memory,
                );
                result.push(Value::Object(create_result_obj(
                    names,
                    index,
                    data_type,
                    decoded_item,
                )))
            }
        };
    }
    result
}

fn decode_internal_struct_item(
    struct_items: Option<&Vec<StructItems>>,
    enum_items: Option<&Vec<EnumItems>>,
    datas: &mut Vec<String>,
    data_type: &str,
    data_index: &mut usize,
    relocated_memory: &Vec<Option<Felt252>>,
) -> Value {
    if let Some(struct_items) = struct_items {
        if let Some(struct_item) = struct_items
            .iter()
            .find(|item| item.name.contains(data_type))
        {
            let types = struct_item
                .members
                .iter()
                .map(|m| m.types.clone())
                .collect::<Vec<String>>();
            let decoded_struct = internal_decode_datas(
                datas,
                &types,
                &struct_item
                    .members
                    .iter()
                    .map(|m| m.names.clone())
                    .collect::<Vec<String>>(),
                Some(struct_items),
                enum_items,
                data_index,
                relocated_memory,
            );

            let result: Map<String, Value> = decoded_struct
                .into_iter()
                .enumerate()
                .map(|(index, item)| (index.to_string(), item))
                .collect();

            // Return the constructed Value::Object
            return Value::Object(result);
        }
    }
    Value::Object(Map::new())
}

fn internal_array(
    datas: &mut Vec<String>,
    data_index: &mut usize,
    struct_items: Option<&Vec<StructItems>>,
    enum_items: Option<&Vec<EnumItems>>,
    inner_type: &str,
    relocated_memory: &Vec<Option<Felt252>>,
    names: &[String],
    index: usize,
    data_type: &str,
) -> Value {
    let mut decoded_array = Vec::new();
    if !relocated_memory.is_empty() && *data_index + 1 < datas.len() {
        let mut extracted_values = Vec::new();
        extracted_values = datas[..*data_index].to_vec();

        let memory_values = extract_memory_values(relocated_memory, datas, data_index);
        extracted_values.extend(memory_values);
        extracted_values.extend(datas[*data_index + 2..].iter().cloned());
        datas.clear();
        datas.extend(extracted_values);
    }

    let array_length = match datas.get(*data_index) {
        Some(length) => length.parse::<usize>().unwrap_or(0),
        None => 0,
    };
    *data_index += 1;

    for _ in 0..array_length {
        let decoded_item = decode_internal_struct_item(
            struct_items,
            enum_items,
            datas,
            inner_type,
            data_index,
            relocated_memory,
        );

        let is_empty = match &decoded_item {
            Value::Array(arr) => arr.is_empty(),
            Value::Object(obj) => obj.is_empty(),
            _ => false,
        };

        if is_empty {
            if let Some(value) = datas.get(*data_index) {
                let data = value.to_string();
                *data_index += 1;
                decoded_array.push(Value::String(data));
            }
        } else {
            decoded_array.push(decoded_item);
        }
    }

    Value::Object(create_result_obj(
        names,
        index,
        data_type,
        Value::Array(decoded_array),
    ))
}

fn extract_memory_values(
    relocated_memory: &Vec<Option<Felt252>>,
    values: &[String],
    value_index: &usize,
) -> Vec<String> {
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
            return memory_values;
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
            return memory_values;
        }
    };
    if end_index < start_index {
        return memory_values;
    }

    if start_index == end_index {
        memory_values.push("1".to_string());
        if let Some(Some(inner_value)) = relocated_memory.get(start_index) {
            memory_values.push(inner_value.to_string());
        }
    } else {
        let size = end_index - start_index;
        memory_values.push(size.to_string());

        for index in start_index..end_index {
            if let Some(Some(inner_value)) = relocated_memory.get(index) {
                memory_values.push(inner_value.to_string());
            }
        }
    }
    memory_values
}

pub fn build_data_items_from_type_declaration(
    type_declaration: &Option<TypeDeclaration>,
    type_declarations: &[TypeDeclaration],
) -> (Option<Vec<EnumItems>>, Option<Vec<StructItems>>) {
    let type_declaration = match type_declaration {
        Some(decl) => decl,
        None => return (None, None),
    };

    let mut enum_items: Vec<EnumItems> = Vec::new();
    let mut variants: Vec<Datas> = Vec::new();
    let mut struct_items: Vec<StructItems> = Vec::new();
    let mut members: Vec<Datas> = Vec::new();

    let struct_name = type_declaration
        .id
        .debug_name
        .as_deref()
        .unwrap_or("")
        .to_string();

    let simplified_struct_name = simplify_type_name(struct_name.as_str());
    for arg in &type_declaration.long_id.generic_args {
        if let GenericArg::Type(concrete_type_id) = arg {
            if let Some(nested_type_declaration) = type_declarations
                .iter()
                .find(|type_decl| type_decl.id.id == concrete_type_id.id)
                .cloned()
            {
                let nested_type_name = nested_type_declaration
                    .id
                    .debug_name
                    .as_deref()
                    .unwrap_or("")
                    .to_string();
                let simplified_nested_type_name = simplify_type_name(nested_type_name.as_str());

                // Handle Enum types only if the main type is an Enum
                if type_declaration.long_id.generic_id == GenericTypeId::from_string("Enum") {
                    variants.push(Datas {
                        names: "".to_string(),
                        types: simplified_nested_type_name.clone(),
                    });
                    let enum_type_name = type_declaration
                        .id
                        .debug_name
                        .as_deref()
                        .unwrap_or("")
                        .to_string();
                    let simplified_enum_type_name = simplify_type_name(enum_type_name.as_str());
                    enum_items = vec![EnumItems {
                        name: simplified_enum_type_name,
                        members: variants.clone(),
                    }];
                }

                // Handle Struct types for both Enum and Struct cases
                members.push(Datas {
                    names: "".to_string(),
                    types: simplified_nested_type_name.clone(),
                });

                let (_, nested_struct_items) = build_data_items_from_type_declaration(
                    &Some(nested_type_declaration),
                    type_declarations,
                );

                if let Some(nested_struct_items) = nested_struct_items {
                    struct_items.extend(nested_struct_items);
                }
            }
        }
    }

    if !members.is_empty() {
        struct_items.push(StructItems {
            name: simplified_struct_name,
            members,
        });
    }

    (Some(enum_items), Some(struct_items))
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
