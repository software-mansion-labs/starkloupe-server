use crate::common::create_result_obj;
use crate::starknet_types::EDataType;
use serde_json::{json, map::Map, Value};
use tracing::{info, warn};
use walnut_shared::{EnumItems, StructItems};

pub fn decode_datas(
    datas: &[String],
    types: &[String],
    names: &[String],
    struct_items: Option<&Vec<StructItems>>,
    enum_items: Option<&Vec<EnumItems>>,
    data_index: &mut usize,
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
                *data_index += 1;
                result.push(Value::Object(create_result_obj(
                    names,
                    index,
                    data_type,
                    Value::String(data),
                )));
            }
            EDataType::Array(inner_type) => {
                let result_value = calldata_array(
                    datas,
                    data_index,
                    struct_items,
                    enum_items,
                    &inner_type.to_string(),
                    names,
                    index,
                    data_type,
                );
                result.push(result_value);
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
                                let decoded_values = decode_datas(
                                    &datas,
                                    &vec![enum_type],
                                    &vec![variant_name.clone()],
                                    struct_items,
                                    Some(enum_items),
                                    data_index,
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

                for inner_type in inner_types {
                    let decoded_inner = decode_datas(
                        datas,
                        &[inner_type.to_string()],
                        names,
                        struct_items,
                        enum_items,
                        data_index,
                    );
                    // Push the first decoded value for other cases
                    if let Some(inner_value) = decoded_inner.first() {
                        decoded_inner_values.push(inner_value.clone());
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
                let decoded_item =
                    decode_struct_item(struct_items, enum_items, datas, data_type, data_index);
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

fn decode_struct_item(
    struct_items: Option<&Vec<StructItems>>,
    enum_items: Option<&Vec<EnumItems>>,
    datas: &[String],
    data_type: &str,
    data_index: &mut usize,
) -> Value {
    if let Some(struct_items) = struct_items {
        if let Some(struct_item) = struct_items
            .iter()
            .find(|item| item.name.contains(data_type))
        {
            let decoded_struct = decode_datas(
                datas,
                &struct_item
                    .members
                    .iter()
                    .map(|m| m.types.clone())
                    .collect::<Vec<String>>(),
                &struct_item
                    .members
                    .iter()
                    .map(|m| m.names.clone())
                    .collect::<Vec<String>>(),
                Some(struct_items),
                enum_items,
                data_index,
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

fn calldata_array(
    datas: &[String],
    data_index: &mut usize,
    struct_items: Option<&Vec<StructItems>>,
    enum_items: Option<&Vec<EnumItems>>,
    inner_type: &str,
    names: &[String],
    index: usize,
    data_type: &str,
) -> Value {
    let mut decoded_array = Vec::new();

    let array_length = match datas.get(*data_index) {
        Some(length) => usize::from_str_radix(length.trim_start_matches("0x"), 16).unwrap_or(0),
        None => 0,
    };
    *data_index += 1;

    for _ in 0..array_length {
        // Decode each item based on its type
        let decoded_item =
            decode_struct_item(struct_items, enum_items, datas, &inner_type, data_index);

        let is_empty = match &decoded_item {
            Value::Array(arr) => arr.is_empty(),
            Value::Object(obj) => obj.is_empty(),
            _ => false,
        };

        if is_empty {
            let data = match datas.get(*data_index) {
                Some(value) => value.to_string(),
                None => {
                    // Handle the case when the data is missing (e.g., logging or fallback)
                    //warn!("No data found at index {}", *data_index);
                    "None".to_string()
                }
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
