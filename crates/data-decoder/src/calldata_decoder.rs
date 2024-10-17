use crate::common::{create_decoded_value, DecodedValue, DecodedValueType};
use crate::starknet_types::EDataType;
use num_traits::cast::ToPrimitive;
use starknet_types_core::felt::Felt;
use std::borrow::Cow;
use std::collections::HashMap;
use walnut_shared::{EnumItems, StructItems};

pub fn decode_calldata(
    datas: &[Felt],
    types: &[Cow<str>],
    names: &[Cow<str>],
    structs: Option<&Vec<StructItems>>,
    enums: Option<&Vec<EnumItems>>,
    data_index: &mut usize,
) -> Option<Vec<DecodedValue>> {
    let mut result = Vec::new();

    for (index, data_type) in types.iter().enumerate() {
        if datas.len() <= *data_index {
            break;
        };

        let e_data_type = EDataType::from_str(data_type, enums);

        let decoded_value = match e_data_type {
            EDataType::Primitive(_) => decode_primitive(
                names.get(index).map(|n| n.as_ref()),
                data_type.as_ref(),
                datas,
                data_index,
            ),
            EDataType::Array(inner_type) => decode_array(
                names.get(index).map(|n| n.as_ref()),
                datas,
                data_index,
                structs,
                enums,
                &inner_type.to_string(),
            ),
            EDataType::UserEnum(_) => decode_enum(datas, data_index, data_type, enums, structs),
            EDataType::Tuple(inner_types) => {
                decode_tuple(datas, data_index, &inner_types, structs, enums)
            }
            EDataType::Struct(_) => decode_struct(
                names.get(index).map(|n| n.as_ref()),
                datas,
                data_index,
                data_type,
                structs,
                enums,
            ),
        };
        if let Some(decoded_value) = decoded_value {
            result.push(decoded_value);
        }
    }

    if result.is_empty() {
        return None;
    }
    Some(result)
}

#[inline(always)]
fn decode_primitive(
    name: Option<&str>,
    data_type: &str,
    datas: &[Felt],
    data_index: &mut usize,
) -> Option<DecodedValue> {
    datas.get(*data_index).map(|value| {
        *data_index += 1;
        create_decoded_value(name, data_type, DecodedValueType::Single(*value))
    })
}

fn decode_array(
    name: Option<&str>,
    datas: &[Felt],
    data_index: &mut usize,
    structs: Option<&Vec<StructItems>>,
    enums: Option<&Vec<EnumItems>>,
    inner_type: &str,
) -> Option<DecodedValue> {
    let array_length = datas.get(*data_index)?.to_usize()?;
    *data_index += 1;

    let mut decoded_elements = Vec::with_capacity(array_length);

    for _ in 0..array_length {
        match decode_calldata(
            &datas[*data_index..],
            &[Cow::Borrowed(inner_type)],
            &[],
            structs,
            enums,
            data_index,
        ) {
            Some(decoded) => decoded_elements.extend(decoded.into_iter().map(|v| v.value)),
            None => {
                if let Some(data) = datas.get(*data_index) {
                    *data_index += 1;
                    decoded_elements.push(DecodedValueType::Single(*data));
                }
            }
        }
    }

    if !decoded_elements.is_empty() {
        return Some(create_decoded_value(
            name,
            inner_type,
            DecodedValueType::Array(decoded_elements),
        ));
    }
    None
}

fn decode_enum(
    datas: &[Felt],
    data_index: &mut usize,
    data_type: &str,
    enums: Option<&Vec<EnumItems>>,
    structs: Option<&Vec<StructItems>>,
) -> Option<DecodedValue> {
    if let Some(variant_index_felt) = datas.get(*data_index) {
        if let Some(variant_index) = variant_index_felt.to_usize() {
            *data_index += 1;

            enums
                .and_then(|e| e.iter().find(|item| item.name == data_type))
                .and_then(|enum_item| enum_item.members.get(variant_index))
                .and_then(|enum_member| {
                    let variant_name = &enum_member.names;
                    let variant_type = &enum_member.types;

                    decode_calldata(
                        &datas[*data_index..],
                        &[Cow::Borrowed(variant_type)],
                        &[Cow::Borrowed(variant_name)],
                        structs,
                        enums,
                        data_index,
                    )
                    .and_then(|mut decoded_variants| decoded_variants.pop())
                    .map(|decoded_variant| {
                        create_decoded_value(
                            Some(variant_name),
                            variant_type,
                            decoded_variant.value,
                        )
                    })
                });
        }
    }
    None
}

fn decode_tuple(
    datas: &[Felt],
    data_index: &mut usize,
    inner_types: &[String],
    structs: Option<&Vec<StructItems>>,
    enums: Option<&Vec<EnumItems>>,
) -> Option<DecodedValue> {
    let mut decoded_values: Vec<DecodedValueType> = Vec::new();
    for inner_type in inner_types {
        if let Some(values) = decode_calldata(
            &datas[*data_index..],
            &[Cow::Borrowed(inner_type)],
            &[],
            structs,
            enums,
            data_index,
        ) {
            decoded_values.extend(values.into_iter().map(|dv| dv.value));
        } else {
            // If decoding fails, return None early
            return None;
        }
    }
    Some(create_decoded_value(
        None,
        "Tuple",
        DecodedValueType::Array(decoded_values),
    ))
}

fn decode_struct(
    name: Option<&str>,
    datas: &[Felt],
    data_index: &mut usize,
    data_type: &str,
    structs: Option<&Vec<StructItems>>,
    enums: Option<&Vec<EnumItems>>,
) -> Option<DecodedValue> {
    let struct_map = decode_struct_map(datas, data_index, data_type, structs, enums);
    (struct_map.is_some()).then(|| create_decoded_value(name, data_type, struct_map.unwrap()))
}

fn decode_struct_map(
    datas: &[Felt],
    data_index: &mut usize,
    data_type: &str,
    structs: Option<&Vec<StructItems>>,
    enums: Option<&Vec<EnumItems>>,
) -> Option<DecodedValueType> {
    if let Some(structs) = structs {
        if let Some(struct_item) = structs.iter().find(|item| item.name.contains(data_type)) {
            // Collect the types and names as Vec<Cow<str>>
            let types: Vec<Cow<str>> = struct_item
                .members
                .iter()
                .map(|m| Cow::Borrowed(m.types.as_str())) // Borrowed &str to Cow<str>
                .collect();

            let names: Vec<Cow<str>> = struct_item
                .members
                .iter()
                .map(|m| Cow::Borrowed(m.names.as_str())) // Borrowed &str to Cow<str>
                .collect();

            // Decode the struct fields
            if let Some(decoded_struct) = decode_calldata(
                &datas[*data_index..],
                &types,
                &names,
                Some(structs),
                enums,
                data_index,
            ) {
                let mut decoded_struct_values = HashMap::new();
                for (key, value) in decoded_struct.into_iter().enumerate() {
                    decoded_struct_values.insert(key.to_string(), value); // Avoid cloning here
                }

                return Some(DecodedValueType::Struct(decoded_struct_values));
            }
        }
    }
    None
}
