use crate::starknet_types::EDataType;
use crate::{create_decoded_value, DecodedValue, DecodedValueType};
use num_traits::cast::ToPrimitive;
use starknet_types_core::felt::Felt;
use std::borrow::Cow;
use std::collections::HashMap;
use walnut_shared::{EnumAbi, StructAbi};

pub fn decode_calldata(
    datas: &[Felt],
    types: &[Cow<str>],
    names: &[Cow<str>],
    structs: Option<&[StructAbi]>,
    enums: Option<&[EnumAbi]>,
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
                data_type,
                &inner_type.to_string(),
            ),
            EDataType::UserEnum(_) => decode_enum(
                names.get(index).map(|n| n.as_ref()),
                datas,
                data_index,
                data_type,
                enums,
                structs,
            ),
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
    structs: Option<&[StructAbi]>,
    enums: Option<&[EnumAbi]>,
    data_type: &str,
    inner_type: &str,
) -> Option<DecodedValue> {
    let array_length = if data_type.ends_with("]") {
        let length = data_type
            .rfind(']')
            .and_then(|idx| data_type[..idx].chars().rev().find(|c| c.is_ascii_digit()))
            .and_then(|c| c.to_digit(10).map(|d| d as usize))?;
        length
    } else {
        let length = datas.get(*data_index)?.to_usize()?;
        *data_index += 1;
        length
    };

    let decoded_elements: Vec<DecodedValueType> = (0..array_length)
        .flat_map(|_| {
            if let Some(decoded) = decode_calldata(
                datas,
                &[Cow::Borrowed(inner_type)],
                &[],
                structs,
                enums,
                data_index,
            ) {
                decoded.into_iter().map(|v| v.value).collect::<Vec<_>>()
            } else if let Some(data) = datas.get(*data_index) {
                *data_index += 1;
                vec![DecodedValueType::Single(*data)]
            } else {
                dbg!("prazan");
                vec![]
            }
        })
        .collect();
    Some(create_decoded_value(
        name,
        data_type,
        DecodedValueType::Array(decoded_elements),
    ))
}

fn decode_enum(
    name: Option<&str>,
    datas: &[Felt],
    data_index: &mut usize,
    data_type: &str,
    enums: Option<&[EnumAbi]>,
    structs: Option<&[StructAbi]>,
) -> Option<DecodedValue> {
    if let Some(variant_index_felt) = datas.get(*data_index) {
        if let Some(enum_item) = enums.and_then(|e| e.iter().find(|item| item.name == data_type)) {
            if let Some(variant_index) = variant_index_felt.to_usize() {
                *data_index += 1;

                if let Some(enum_member) = enum_item.parameters.get(variant_index) {
                    let variant_name = &enum_member.name;
                    let variant_type = &enum_member.type_name;

                    if variant_type.trim().is_empty() {
                        return Some(create_decoded_value(
                            name,
                            data_type,
                            DecodedValueType::String(variant_name.to_string()),
                        ));
                    }

                    return decode_calldata(
                        datas,
                        &[Cow::Borrowed(variant_type)],
                        &[Cow::Borrowed(variant_name)],
                        structs,
                        enums,
                        data_index,
                    )
                    .and_then(|mut decoded_variants| decoded_variants.pop())
                    .map(|decoded_variant| {
                        create_decoded_value(
                            name,
                            format!("{}: {}", variant_name, variant_type).as_str(),
                            decoded_variant.value,
                        )
                    });
                }
            }
            return Some(create_decoded_value(
                name,
                data_type,
                DecodedValueType::Single(*variant_index_felt),
            ));
        }
    }
    None
}

fn decode_tuple(
    datas: &[Felt],
    data_index: &mut usize,
    inner_types: &[String],
    structs: Option<&[StructAbi]>,
    enums: Option<&[EnumAbi]>,
) -> Option<DecodedValue> {
    let mut tuple_fields: Vec<DecodedValue> = Vec::new();

    for inner_type in inner_types {
        let decoded_field = decode_calldata(
            datas,
            &[Cow::Borrowed(inner_type)],
            &[],
            structs,
            enums,
            data_index,
        )?
        .into_iter()
        .next()?;

        tuple_fields.push(decoded_field);
    }
    let mut tuple_map = HashMap::new();
    for (i, field) in tuple_fields.into_iter().enumerate() {
        tuple_map.insert(i, field);
    }

    Some(create_decoded_value(
        None,
        &inner_types.join(", "),
        DecodedValueType::Struct(tuple_map),
    ))
}

fn decode_struct(
    name: Option<&str>,
    datas: &[Felt],
    data_index: &mut usize,
    data_type: &str,
    structs: Option<&[StructAbi]>,
    enums: Option<&[EnumAbi]>,
) -> Option<DecodedValue> {
    let struct_map = decode_struct_map(datas, data_index, data_type, structs, enums);
    (struct_map.is_some()).then(|| create_decoded_value(name, data_type, struct_map.unwrap()))
}

fn decode_struct_map(
    datas: &[Felt],
    data_index: &mut usize,
    data_type: &str,
    structs: Option<&[StructAbi]>,
    enums: Option<&[EnumAbi]>,
) -> Option<DecodedValueType> {
    if let Some(structs) = structs {
        if let Some(struct_item) = structs.iter().find(|item| item.name.contains(data_type)) {
            // Collect the types and names as Vec<Cow<str>>
            let types: Vec<Cow<str>> = struct_item
                .parameters
                .iter()
                .map(|m| Cow::Borrowed(m.type_name.as_str())) // Borrowed &str to Cow<str>
                .collect();

            let names: Vec<Cow<str>> = struct_item
                .parameters
                .iter()
                .map(|m| Cow::Borrowed(m.name.as_str())) // Borrowed &str to Cow<str>
                .collect();

            if let Some(decoded_struct) =
                decode_calldata(datas, &types, &names, Some(structs), enums, data_index)
            {
                let mut decoded_struct_values = HashMap::new();
                for (key, value) in decoded_struct.into_iter().enumerate() {
                    decoded_struct_values.insert(key, value);
                }

                return Some(DecodedValueType::Struct(decoded_struct_values));
            }
        }
    }
    None
}
