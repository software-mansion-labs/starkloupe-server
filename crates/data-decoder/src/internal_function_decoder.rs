use crate::common::create_result_obj;
use crate::starknet_types::EDataType;
use serde_json::{json, map::Map, Value};
use starknet_types_core::felt::Felt as Felt252;
use tracing::{info, warn};
use walnut_shared::{EnumItems, StructItems};

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
            EDataType::System(_) => {
                let values: Value = datas[*data_index..]
                    .iter()
                    .map(|data| {
                        let value = json!(data);
                        *data_index += 1;
                        value
                    })
                    .collect();
                result.push(Value::Object(create_result_obj(
                    names, index, data_type, values,
                )));
            }
            EDataType::Primitive(_) => {
                let data = match datas.get(*data_index) {
                    Some(value) => value.clone(),
                    None => {
                        warn!("No data found at index {}", *data_index);
                        "Invalid data".to_string()
                    }
                };
                dbg!("Increase data_index");
                *data_index += 1;
                dbg!(&data);
                dbg!(&data_index);
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
                        if enum_item.name.contains(&*data_type) {
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

                // Check if there's only one inner type and more than one data element
                // This happen from sierra return "type":
                // "core::panics::PanicResult::<(core::bool,)>" -> Tuple<core::bool>",
                if inner_types.len() == 1 && datas.len() > 1 {
                    if let Some(last_data) = datas.last() {
                        // Push the last element from datas into decoded_inner_values
                        decoded_inner_values.push(json!(last_data));
                    }
                } else {
                    for inner_type in inner_types {
                        if inner_type.starts_with("Option::<") {
                            dbg!(&inner_type);
                            if let Some(stripped_inner) = inner_type
                                .strip_prefix("Option::<")
                                .and_then(|s| s.strip_suffix('>'))
                            {
                                dbg!(&datas);
                                dbg!(&data_index);
                                dbg!(&stripped_inner);
                                let _removed = datas.remove(*data_index);
                                dbg!(&datas);
                                let decoded_inner = internal_decode_datas(
                                    datas,
                                    &[stripped_inner.to_string()],
                                    names,
                                    struct_items,
                                    enum_items,
                                    data_index,
                                    relocated_memory,
                                );
                                if let Some(inner_value) = decoded_inner.first() {
                                    decoded_inner_values.push(inner_value.clone());
                                }
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
                            // Push the first decoded value for other cases
                            if let Some(inner_value) = decoded_inner.first() {
                                decoded_inner_values.push(inner_value.clone());
                            }
                        }
                    }
                }
                let mut decoded_data = Map::new();
                for (index, item) in decoded_inner_values.iter().enumerate() {
                    decoded_data.insert(index.to_string(), item.clone()); // Insert with index as the key
                }

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

            let mut result = Map::new();
            for (index, item) in decoded_struct.iter().enumerate() {
                result.insert(index.to_string(), item.clone()); // Insert with index as the key
            }

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

        let memory_values = extract_memory_values(relocated_memory.as_ref(), datas, data_index);
        extracted_values.extend(memory_values);
        extracted_values.extend(datas[*data_index + 2..].iter().cloned());
        datas.clear();
        datas.extend(extracted_values);
    }

    let array_length = match datas.get(*data_index) {
        Some(str_value) => str_value.parse::<usize>().unwrap_or(0),
        None => 0, // Fallback value if no string is found
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
            let data = match datas.get(*data_index) {
                Some(value) => value.to_string(),
                None => "None".to_string(),
            };
            *data_index += 1;
            decoded_array.push(Value::String(data));
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

pub fn extract_memory_values(
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

        // Push the values from the memory, from start_index to end_index
        for index in start_index..end_index {
            if let Some(Some(inner_value)) = relocated_memory.get(index) {
                memory_values.push(inner_value.to_string());
            }
        }
    }
    memory_values
}
