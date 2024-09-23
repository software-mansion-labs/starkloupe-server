mod starknet_types;
use serde_json::{json, map::Map, Value};
use starknet_types::{EDataType, EEnumType};
use tracing::{info, warn};
use walnut_shared::{EnumItems, StructItems};

pub fn decode_datas(
    datas: &Vec<String>,
    types: &Vec<String>,
    names: &Vec<String>,
    struct_items: Option<&Vec<StructItems>>,
    enum_items: Option<&Vec<EnumItems>>,
    data_index: &mut usize,
    expect_array_with_length: bool,
) -> Vec<Value> {
    let mut result = Vec::new();

    for (index, data_type) in types.iter().enumerate() {
        if datas.len() <= *data_index {
            break;
        };
        let e_data_type = EDataType::from_str(data_type, enum_items);
        match e_data_type {
            EDataType::System(_) => {
                let values: Vec<serde_json::Value> = datas[*data_index..]
                    .iter()
                    .map(|data| {
                        let value = json!(data);
                        *data_index += 1;
                        value
                    })
                    .collect();
                result.push(Value::Object(create_result_obj(
                    names,
                    index,
                    data_type,
                    ValueType::Array(values),
                )));
            }
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
                if expect_array_with_length {
                    let result_value = calldata_array(
                        datas,
                        data_index,
                        struct_items,
                        enum_items,
                        &inner_type.to_string(),
                        expect_array_with_length,
                        names,
                        index,
                        data_type,
                    );
                    result.push(result_value);
                } else {
                    let result_value = internal_function_array(
                        datas,
                        data_index,
                        struct_items,
                        enum_items,
                        &inner_type.to_string(),
                        expect_array_with_length,
                        names,
                        index,
                        data_type,
                    );
                    result.push(result_value);
                }
            }
            EDataType::SystemEnum(_) | EDataType::UserEnum(_) => {
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
                        if enum_item.name == *data_type {
                            if let Some(enum_member_item) = enum_item.members.get(enum_index) {
                                let variant_name = enum_member_item.names.clone();
                                let enum_type = enum_member_item.types.clone();
                                let decoded_values = decode_datas(
                                    datas,
                                    &vec![enum_type],
                                    &vec![variant_name.clone()],
                                    struct_items,
                                    Some(enum_items),
                                    data_index,
                                    expect_array_with_length,
                                );

                                result.push(Value::Object(create_result_obj(
                                    names,
                                    index,
                                    data_type,
                                    ValueType::Array(decoded_values),
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
                        let decoded_inner = decode_datas(
                            datas,
                            &vec![inner_type.to_string()],
                            names,
                            struct_items,
                            enum_items,
                            data_index,
                            expect_array_with_length,
                        );
                        // Push the first decoded value for other cases
                        if let Some(inner_value) = decoded_inner.first() {
                            decoded_inner_values.push(inner_value.clone());
                        }
                    }
                }
                result.push(Value::Object(create_result_obj(
                    names,
                    index,
                    data_type,
                    ValueType::Array(decoded_inner_values),
                )));
            }
            EDataType::Struct(_) => {
                let decoded_item = decode_struct_item(
                    struct_items,
                    enum_items,
                    datas,
                    data_type,
                    data_index,
                    expect_array_with_length,
                );
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
    struct_items: Option<&Vec<StructItems>>,
    enum_items: Option<&Vec<EnumItems>>,
    datas: &Vec<String>,
    data_type: &str,
    data_index: &mut usize,
    expect_array_with_length: bool,
) -> Vec<Value> {
    if let Some(struct_items) = struct_items {
        if let Some(struct_item) = struct_items.iter().find(|item| item.name == *data_type) {
            return decode_datas(
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
                Some(struct_items),
                enum_items,
                data_index,
                expect_array_with_length,
            );
        }
    }
    Vec::new()
}

fn calldata_array(
    datas: &Vec<String>,
    data_index: &mut usize,
    struct_items: Option<&Vec<StructItems>>,
    enum_items: Option<&Vec<EnumItems>>,
    inner_type: &str,
    expect_array_with_length: bool,
    names: &Vec<String>,
    index: usize,
    data_type: &str,
) -> Value {
    let mut decoded_array = Vec::new();

    // Handle case where array length is specified as first data in calldata
    let array_length_hex = datas.get(*data_index).map(|s| s.as_str()).unwrap_or("0");
    let array_length =
        usize::from_str_radix(array_length_hex.trim_start_matches("0x"), 16).unwrap_or(0);
    *data_index += 1;

    for _ in 0..array_length {
        // Decode each item based on its type
        let mut decoded_item = decode_struct_item(
            struct_items,
            enum_items,
            datas,
            &inner_type.to_string(),
            data_index,
            expect_array_with_length,
        );

        if decoded_item.is_empty() {
            // For primitive types, include only the value
            let data = match datas.get(*data_index) {
                Some(value) => value.to_string(),
                None => {
                    // Handle the case when the data is missing (e.g., logging or fallback)
                    warn!("No data found at index {}", *data_index);
                    "".to_string() // Return an empty string or provide a default value
                }
            };
            *data_index += 1;
            decoded_item = vec![json!({"value": data})];
        }

        if decoded_item.len() == 1 {
            decoded_array.push(decoded_item[0]["value"].clone());
        } else {
            decoded_array.push(json!(decoded_item));
        }
    }

    Value::Object(create_result_obj(
        names,
        index,
        data_type,
        ValueType::Array(decoded_array),
    ))
}

fn internal_function_array(
    datas: &Vec<String>,
    data_index: &mut usize,
    struct_items: Option<&Vec<StructItems>>,
    enum_items: Option<&Vec<EnumItems>>,
    inner_type: &str,
    expect_array_with_length: bool,
    names: &Vec<String>,
    index: usize,
    data_type: &str,
) -> Value {
    let mut decoded_array = Vec::new();

    // Collect values until the first '0' or '1' or end of the data
    while *data_index < datas.len() && datas[*data_index] != "0" && datas[*data_index] != "1" {
        let data = match datas.get(*data_index) {
            Some(value) => value.to_string(),
            None => {
                // Handle the case when the data is missing (e.g., logging or fallback)
                warn!("No data found at index {}", *data_index);
                "".to_string() // Return an empty string or provide a default value
            }
        };
        decoded_array.push(json!(data));
        *data_index += 1;
    }

    let struct_items_decoded = decode_struct_item(
        struct_items,
        enum_items,
        datas,
        &inner_type.to_string(),
        data_index,
        expect_array_with_length,
    );

    let combined_decoded: Vec<Value> = decoded_array
        .into_iter()
        .chain(struct_items_decoded.into_iter())
        .collect();

    Value::Object(create_result_obj(
        names,
        index,
        data_type,
        ValueType::Array(combined_decoded),
    ))
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
