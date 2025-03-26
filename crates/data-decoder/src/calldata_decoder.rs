use crate::starknet_types::EDataType;
use crate::{create_decoded_value_by_type, DecodedValue, DecodedValueType};
use num_traits::cast::ToPrimitive;
use starknet_types_core::felt::Felt;
use std::borrow::Cow;
use std::collections::HashMap;
use walnut_shared::abi::{Enum, Struct};

type Structs<'a> = &'a [Struct];
type Enums<'a> = &'a [Enum];

pub fn decode_calldata(
    data: &[Felt],
    types: &[Cow<str>],
    names: &[Cow<str>],
    structs: Structs<'_>,
    enums: Enums<'_>,
) -> Option<Vec<DecodedValue>> {
    let mut result = Vec::with_capacity(types.len());
    let mut current_data = data;

    for (idx, ty) in types.iter().enumerate() {
        let decoded = decode_type(
            &mut current_data,
            ty.as_ref(),
            names.get(idx).map(|n| n.as_ref()),
            structs,
            enums,
        )?;
        result.push(decoded);
    }

    if !current_data.is_empty() {
        return None;
    }

    Some(result)
}

fn decode_type(
    data: &mut &[Felt],
    ty: &str,
    name: Option<&str>,
    structs: Structs<'_>,
    enums: Enums<'_>,
) -> Option<DecodedValue> {
    let etype = EDataType::from_str(ty, Some(enums));
    match etype {
        EDataType::Primitive(_) => {
            let (value, rest) = data.split_first()?;
            *data = rest;
            Some(create_decoded_value_by_type(
                name,
                ty,
                DecodedValueType::Single(*value),
            ))
        }

        EDataType::Array(_inner) => {
            // Extract inner type string directly from the original type
            let inner_ty_str = if let Some(inner_ty) =
                ty.strip_prefix("Array<").and_then(|s| s.strip_suffix('>'))
            {
                inner_ty
            } else if let Some(inner_ty) =
                ty.strip_prefix("Span<").and_then(|s| s.strip_suffix('>'))
            {
                inner_ty
            } else if ty.ends_with("[]") {
                ty.strip_suffix("[]").unwrap()
            } else if let Some(stripped) = ty.strip_prefix('[').and_then(|s| s.strip_suffix(']')) {
                stripped.split(';').next().unwrap().trim()
            } else {
                return None;
            };
            // Get array length (fixed or dynamic)
            let array_length =
                if let Some(stripped) = ty.strip_prefix('[').and_then(|s| s.strip_suffix(']')) {
                    stripped
                        .split(';')
                        .nth(1)
                        .unwrap()
                        .trim()
                        .parse::<usize>()
                        .ok()?
                } else {
                    let (len_felt, rest) = data.split_first()?;
                    let len = len_felt.to_usize()?;
                    *data = rest;
                    len
                };

            let mut elements = Vec::with_capacity(array_length);

            // Decode each element using the original inner type string
            for _ in 0..array_length {
                let element = decode_type(data, inner_ty_str, None, structs, enums)?;
                elements.push(element.value);
            }

            Some(create_decoded_value_by_type(
                name,
                ty,
                DecodedValueType::Array(elements),
            ))
        }
        EDataType::UserEnum(_) => {
            let (variant_idx, rest) = data.split_first()?;
            let variant_idx = variant_idx.to_usize()?;
            *data = rest;

            let enum_def = enums.iter().find(|e| e.name == ty)?;
            let variant = enum_def.variants.get(variant_idx)?;

            let value = if variant.ty.is_empty() {
                DecodedValueType::String(variant.name.clone())
            } else {
                let decoded = decode_type(data, &variant.ty, None, structs, enums)?;
                decoded.value
            };

            Some(create_decoded_value_by_type(name, ty, value))
        }
        EDataType::Tuple(inners) => {
            let mut elements = Vec::with_capacity(inners.len());
            let mut tuple_data = *data;

            for inner in inners {
                let decoded = decode_type(&mut tuple_data, &inner, None, structs, enums)?;
                elements.push(decoded.value);
            }

            *data = tuple_data;
            Some(create_decoded_value_by_type(
                name,
                ty,
                DecodedValueType::Array(elements),
            ))
        }
        EDataType::Struct(_) => {
            let struct_def = structs.iter().find(|s| s.name == ty)?;
            let mut fields = HashMap::new();
            let mut struct_data = *data;

            for (index, member) in struct_def.members.iter().enumerate() {
                let decoded = decode_type(
                    &mut struct_data,
                    &member.ty,
                    Some(&member.name),
                    structs,
                    enums,
                )?;

                fields.insert(index, decoded);
            }

            *data = struct_data;
            Some(create_decoded_value_by_type(
                name,
                ty,
                DecodedValueType::Struct(fields),
            ))
        }
    }
}
