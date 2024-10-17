use crate::common::{
    create_decoded_value, simplify_type_name, skip_builtin_type_declaration, DecodedValue,
    DecodedValueType,
};
use cairo_lang_sierra::ids::ConcreteTypeId;
use cairo_lang_sierra::program::{GenericArg, TypeDeclaration};
use num_traits::cast::ToPrimitive;
use starknet_types_core::felt::Felt;
use std::collections::HashMap;

pub fn decode_internal_datas(
    values: &[Felt],
    type_id: &ConcreteTypeId,
    type_declaration_map: &HashMap<ConcreteTypeId, TypeDeclaration>,
    relocated_memory: &[Option<Felt>],
    data_index: &mut usize,
) -> Option<DecodedValue> {
    let type_declaration = type_declaration_map.get(type_id)?;
    let generic_type_id = &type_declaration.long_id.generic_id;
    let debug_name = simplify_type_name(type_declaration.id.debug_name.as_deref().unwrap_or(""));

    if skip_builtin_type_declaration(debug_name.as_str()) {
        return None;
    }

    if type_declaration.long_id.generic_args.is_empty() {
        return values.get(*data_index).map(|value| {
            *data_index += 1;
            create_decoded_value(None, &debug_name, DecodedValueType::Single(*value))
        });
    }

    match generic_type_id.0.as_str() {
        "Enum" => decode_enum(
            values,
            &debug_name,
            type_declaration,
            data_index,
            type_declaration_map,
            relocated_memory,
        ),
        "Struct" => {
            if debug_name.starts_with("Span<") {
                // Handle Span types specially
                decode_span(
                    values,
                    &debug_name,
                    type_declaration,
                    data_index,
                    type_declaration_map,
                    relocated_memory,
                )
            } else {
                decode_struct(
                    values,
                    &debug_name,
                    type_declaration,
                    data_index,
                    type_declaration_map,
                    relocated_memory,
                )
            }
        }
        "Array" => decode_array(
            type_declaration,
            &debug_name,
            relocated_memory,
            values,
            data_index,
            type_declaration_map,
        ),
        "Snapshot" => decode_snapshot(
            type_declaration,
            values,
            data_index,
            relocated_memory,
            type_declaration_map,
        ),
        _ => None,
    }
}

fn decode_enum(
    values: &[Felt],
    debug_name: &str,
    type_declaration: &TypeDeclaration,
    data_index: &mut usize,
    type_declaration_map: &HashMap<ConcreteTypeId, TypeDeclaration>,
    relocated_memory: &[Option<Felt>],
) -> Option<DecodedValue> {
    if debug_name == "bool" {
        if let Some(bool_value) = values.get(*data_index) {
            *data_index += 1;
            return Some(create_decoded_value(
                None,
                "bool",
                DecodedValueType::Bool(*bool_value != Felt::ZERO),
            ));
        }
    }
    if let Some(value) = values.get(*data_index) {
        if let Some(index) = value.to_usize() {
            if let Some(GenericArg::Type(concrete_type_id)) =
                type_declaration.long_id.generic_args.get(index + 1)
            {
                *data_index += 1;
                let decoded_enum_value = decode_internal_datas(
                    values,
                    concrete_type_id,
                    type_declaration_map,
                    relocated_memory,
                    data_index,
                );
                return decoded_enum_value;
            }
        }
    }
    None
}

fn decode_span(
    values: &[Felt],
    debug_name: &str,
    type_declaration: &TypeDeclaration,
    data_index: &mut usize,
    type_declaration_map: &HashMap<ConcreteTypeId, TypeDeclaration>,
    relocated_memory: &[Option<Felt>],
) -> Option<DecodedValue> {
    // Span is a struct with one field, which is the inner array
    // The inner array type is the second generic argument
    if let Some(GenericArg::Type(inner_type_id)) = type_declaration.long_id.generic_args.get(1) {
        decode_internal_datas(
            values,
            inner_type_id,
            type_declaration_map,
            relocated_memory,
            data_index,
        )
        .map(|decoded_value| create_decoded_value(None, debug_name, decoded_value.value))
    } else {
        None
    }
}

fn decode_struct(
    values: &[Felt],
    debug_name: &str,
    type_declaration: &TypeDeclaration,
    data_index: &mut usize,
    type_declaration_map: &HashMap<ConcreteTypeId, TypeDeclaration>,
    relocated_memory: &[Option<Felt>],
) -> Option<DecodedValue> {
    let mut decoded_struct_values =
        HashMap::with_capacity(type_declaration.long_id.generic_args.len());

    for (i, arg) in type_declaration.long_id.generic_args.iter().enumerate() {
        if let GenericArg::Type(concrete_type_id) = arg {
            if let Some(decoded_value) = decode_internal_datas(
                values,
                concrete_type_id,
                type_declaration_map,
                relocated_memory,
                data_index,
            ) {
                if let DecodedValueType::Struct(inner_map) = decoded_value.value {
                    for (key, value) in inner_map {
                        decoded_struct_values.insert(key, value);
                    }
                } else {
                    decoded_struct_values.insert(i.to_string(), decoded_value);
                }
            }
        }
    }

    (!decoded_struct_values.is_empty()).then(|| {
        create_decoded_value(
            None,
            debug_name,
            DecodedValueType::Struct(decoded_struct_values),
        )
    })
}

fn decode_array(
    type_declaration: &TypeDeclaration,
    debug_name: &str,
    relocated_memory: &[Option<Felt>],
    values: &[Felt],
    data_index: &mut usize,
    type_declaration_map: &HashMap<ConcreteTypeId, TypeDeclaration>,
) -> Option<DecodedValue> {
    let current_index: usize = *data_index;

    let (extracted_length, memory_values) =
        extract_memory_values(relocated_memory, values, data_index);

    if memory_values.is_empty() {
        return None;
    }
    *data_index = current_index + 2;
    let decoded_array_values = decode_array_elements_from_memory(
        type_declaration,
        extracted_length,
        &memory_values,
        relocated_memory,
        type_declaration_map,
    );

    let decoded_value = create_decoded_value(
        None,
        debug_name,
        DecodedValueType::Array(decoded_array_values),
    );

    Some(decoded_value)
}

fn decode_snapshot(
    type_declaration: &TypeDeclaration,
    values: &[Felt],
    data_index: &mut usize,
    relocated_memory: &[Option<Felt>],
    type_declaration_map: &HashMap<ConcreteTypeId, TypeDeclaration>,
) -> Option<DecodedValue> {
    if let Some(GenericArg::Type(inner_type_id)) = type_declaration.long_id.generic_args.first() {
        decode_internal_datas(
            values,
            inner_type_id,
            type_declaration_map,
            relocated_memory,
            data_index,
        )
    } else {
        None
    }
}

fn decode_array_elements_from_memory(
    type_declaration: &TypeDeclaration,
    array_length: usize,
    memory_values: &[Felt],
    relocated_memory: &[Option<Felt>],
    type_declaration_map: &HashMap<ConcreteTypeId, TypeDeclaration>,
) -> Vec<DecodedValueType> {
    let mut decoded_array_values = Vec::with_capacity(array_length);
    if let Some(GenericArg::Type(concrete_type_id)) = type_declaration.long_id.generic_args.first()
    {
        let mut index: usize = 0;
        for _ in 0..array_length {
            if index >= memory_values.len() {
                break;
            }
            let values_slice = &memory_values[index..];
            let mut local_data_index = 0;

            if let Some(decoded_value) = decode_internal_datas(
                values_slice,
                concrete_type_id,
                type_declaration_map,
                relocated_memory,
                &mut local_data_index,
            ) {
                decoded_array_values.push(decoded_value.value);
                index += local_data_index;
            }
        }
    }

    decoded_array_values
}

fn extract_memory_values(
    relocated_memory: &[Option<Felt>],
    values: &[Felt],
    value_index: &usize,
) -> (usize, Vec<Felt>) {
    if let Some(slice) = values.get(*value_index..*value_index + 2) {
        if let (Some(start_index), Some(end_index)) = (slice[0].to_usize(), slice[1].to_usize()) {
            if end_index > start_index {
                let size = end_index - start_index;
                let memory_values: Vec<Felt> = (start_index..=end_index)
                    .filter_map(|index| relocated_memory.get(index).and_then(|&v| v))
                    .collect();

                return (size, memory_values);
            }
            if end_index == start_index {
                return (
                    1,
                    vec![relocated_memory
                        .get(start_index)
                        .and_then(|&v| v)
                        .unwrap_or(Felt::ZERO)],
                );
            }
        }
    }
    (0, vec![])
}
