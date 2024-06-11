mod starknet_types;
use serde_json::{json, map::Map, Value};
use starknet_types::EDataType;
use walnut_shared::StructItems;

pub fn decode_datas(
    datas: &Vec<String>,
    types: &Vec<String>,
    names: &Vec<String>,
    struct_items: &Vec<StructItems>,
    data_index: &mut usize,
) -> Vec<Value> {
    let mut result = Vec::new();

    for (index, data_type) in types.iter().enumerate() {
        if datas.len() <= *data_index {
            break;
        };
        let e_data_type = EDataType::from_str(data_type);
        match e_data_type {
            EDataType::Primitive(_) => {
                let data = &datas[*data_index];
                *data_index += 1;
                result.push(Value::Object(create_result_obj(
                    names,
                    index,
                    data_type,
                    ValueType::Single(data.to_string()),
                )))
            }
            EDataType::Array(inner_type) => {
                let array_lenght_hex = datas.get(*data_index).unwrap().trim_start_matches("0x");
                let array_length = usize::from_str_radix(array_lenght_hex, 16).unwrap();
                *data_index += 1;
                let mut decoded_array = Vec::new();
                for _ in 0..array_length {
                    // For the struct type include the type and name of struct memebers
                    let mut decoded_item = decode_struct_item(
                        struct_items,
                        datas,
                        &inner_type.to_string(),
                        data_index,
                    );
                    if decoded_item.is_empty() {
                        //For the primitive types include only the value
                        let data = datas[*data_index].to_string();
                        *data_index += 1;
                        decoded_item = vec![json!({"value": data})];
                    }
                    if decoded_item.len() == 1 {
                        decoded_array.push(decoded_item[0]["value"].clone());
                    } else {
                        decoded_array.push(json!(decoded_item));
                    }
                }

                result.push(Value::Object(create_result_obj(
                    names,
                    index,
                    data_type,
                    ValueType::Array(decoded_array),
                )))
            }
            EDataType::Struct(_) => {
                let decoded_item = decode_struct_item(struct_items, datas, data_type, data_index);
                result.push(Value::Object(create_result_obj(
                    names,
                    index,
                    data_type,
                    ValueType::Array(decoded_item),
                )))
            }
        };
    }
    result
}

fn decode_struct_item(
    struct_items: &Vec<StructItems>,
    datas: &Vec<String>,
    data_type: &str,
    data_index: &mut usize,
) -> Vec<Value> {
    if let Some(struct_item) = struct_items.iter().find(|item| item.name == *data_type) {
        decode_datas(
            datas,
            &struct_item
                .members
                .iter()
                .map(|m| m.types.clone())
                .collect(),
            &struct_item
                .members
                .iter()
                .map(|m| m.names.clone())
                .collect(),
            struct_items,
            data_index,
        )
    } else {
        Vec::new()
    }
}

enum ValueType {
    Single(String),
    Array(Vec<serde_json::Value>),
}

fn create_result_obj(
    names: &Vec<String>,
    index: usize,
    data_type: &str,
    value: ValueType,
) -> Map<String, Value> {
    let mut result_obj = Map::new();

    if !names.is_empty() && !names[index].is_empty() {
        result_obj.insert("name".to_string(), json!(names[index]));
    }
    result_obj.insert("type".to_string(), json!(data_type));

    match value {
        ValueType::Single(v) => {
            result_obj.insert("value".to_string(), json!(v));
        }
        ValueType::Array(v) => {
            result_obj.insert("value".to_string(), json!(v));
        }
    }

    result_obj
}
