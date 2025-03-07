use crate::utils::{simplify_type_name, skip_builtin_type_declaration};
use crate::{create_decoded_value_by_type, DecodedValue, DecodedValueType};
use cairo_lang_sierra::ids::ConcreteTypeId;
use cairo_lang_sierra::program::{GenericArg, TypeDeclaration};
use cairo_lang_sierra_type_size::TypeSizeMap;
use num_traits::cast::ToPrimitive;
use starknet_types_core::felt::Felt;
use std::collections::HashMap;

pub fn decode_internal_datas(
    values: &[Felt],
    type_id: &ConcreteTypeId,
    type_declaration_map: &HashMap<ConcreteTypeId, TypeDeclaration>,
    type_sizes: &TypeSizeMap,
    relocated_memory: &[Option<Felt>],
    data_index: &mut usize,
) -> Option<DecodedValue> {
    let type_declaration = type_declaration_map.get(type_id)?;
    let type_size = type_sizes.get(type_id)?;
    let generic_type_id = &type_declaration.long_id.generic_id;
    let generic_args = &type_declaration.long_id.generic_args;
    let debug_name = simplify_type_name(type_declaration.id.debug_name.as_deref().unwrap_or(""));

    if skip_builtin_type_declaration(debug_name.as_str()) {
        return None;
    }

    if generic_args.is_empty() {
        return values.get(*data_index).map(|value| {
            *data_index += 1;
            create_decoded_value_by_type(None, &debug_name, DecodedValueType::Single(*value))
        });
    }
    match generic_type_id.0.as_str() {
        "Enum" => decode_enum(
            values,
            &debug_name,
            generic_args,
            data_index,
            type_declaration_map,
            type_sizes,
            type_size,
            relocated_memory,
        ),
        "Struct" => decode_struct(
            values,
            &debug_name,
            generic_args,
            data_index,
            type_declaration_map,
            type_sizes,
            relocated_memory,
        ),
        "Array" => decode_array(
            generic_args,
            &debug_name,
            relocated_memory,
            values,
            data_index,
            type_declaration_map,
            type_sizes,
        ),
        "Snapshot" => decode_snapshot(
            generic_args,
            values,
            data_index,
            relocated_memory,
            type_declaration_map,
            type_sizes,
        ),
        // https://docs.swmansion.com/scarb/corelib/core-zeroable-NonZero.html
        "NonZero" => {
            if let Some(GenericArg::Type(inner_type_id)) = generic_args.first() {
                decode_internal_datas(
                    values,
                    inner_type_id,
                    type_declaration_map,
                    type_sizes,
                    relocated_memory,
                    data_index,
                )
            } else {
                None
            }
        }
        _ => None,
    }
}

fn decode_enum(
    values: &[Felt],
    debug_name: &str,
    generic_args: &[GenericArg],
    data_index: &mut usize,
    type_declaration_map: &HashMap<ConcreteTypeId, TypeDeclaration>,
    type_sizes: &TypeSizeMap,
    type_size: &i16,
    relocated_memory: &[Option<Felt>],
) -> Option<DecodedValue> {
    let total_variants = generic_args.len().saturating_sub(1);
    if let Some(value) = values.get(*data_index) {
        let variant_selector = value.to_usize()?;
        let variant_index = if total_variants > 2 {
            total_variants.saturating_sub((variant_selector + 1) / 2)
        } else {
            variant_selector
        };

        let concrete_type_id = match generic_args.get(variant_index + 1) {
            Some(GenericArg::Type(concrete_type_id)) => concrete_type_id,
            _ => return None,
        };

        let variant_type_size = type_sizes.get(concrete_type_id)?;

        let starting_values_index = type_size.saturating_sub(*variant_type_size);
        *data_index += starting_values_index.to_usize()?;

        if let Some("Unit") = concrete_type_id.debug_name.as_deref() {
            return Some(create_decoded_value_by_type(
                None,
                debug_name,
                DecodedValueType::Single(Felt::from(variant_index)),
            ));
        }

        let decoded_value = decode_internal_datas(
            values,
            concrete_type_id,
            type_declaration_map,
            type_sizes,
            relocated_memory,
            data_index,
        )?;
        return Some(create_decoded_value_by_type(
            None,
            debug_name,
            DecodedValueType::Enum(debug_name.to_string(), Box::new(decoded_value)),
        ));
    } else {
        None
    }
}

fn decode_struct(
    values: &[Felt],
    debug_name: &str,
    generic_args: &[GenericArg],
    data_index: &mut usize,
    type_declaration_map: &HashMap<ConcreteTypeId, TypeDeclaration>,
    type_sizes: &TypeSizeMap,
    relocated_memory: &[Option<Felt>],
) -> Option<DecodedValue> {
    let simplified_debug_name = simplify_type_name(debug_name);
    if simplified_debug_name.starts_with("Span<") {
        decode_span(
            values,
            &simplified_debug_name,
            generic_args,
            data_index,
            type_declaration_map,
            type_sizes,
            relocated_memory,
        )
    } else {
        decode_standard_struct(
            values,
            &simplified_debug_name,
            generic_args,
            data_index,
            type_declaration_map,
            type_sizes,
            relocated_memory,
        )
    }
}

fn decode_span(
    values: &[Felt],
    debug_name: &str,
    generic_args: &[GenericArg],
    data_index: &mut usize,
    type_declaration_map: &HashMap<ConcreteTypeId, TypeDeclaration>,
    type_sizes: &TypeSizeMap,
    relocated_memory: &[Option<Felt>],
) -> Option<DecodedValue> {
    // Span is a struct with one field, which is the inner array
    // The inner array type is the second generic argument
    if let Some(GenericArg::Type(inner_type_id)) = generic_args.get(1) {
        decode_internal_datas(
            values,
            inner_type_id,
            type_declaration_map,
            type_sizes,
            relocated_memory,
            data_index,
        )
        .map(|decoded_value| create_decoded_value_by_type(None, debug_name, decoded_value.value))
    } else {
        None
    }
}
fn decode_standard_struct(
    values: &[Felt],
    debug_name: &str,
    generic_args: &[GenericArg],
    data_index: &mut usize,
    type_declaration_map: &HashMap<ConcreteTypeId, TypeDeclaration>,
    type_sizes: &TypeSizeMap,
    relocated_memory: &[Option<Felt>],
) -> Option<DecodedValue> {
    // This is to avoid decoding the ContractState struct, because there is 17 members
    //and values are empty, as the values array in the case of ContractState is empty
    if debug_name == "ContractState" || debug_name.contains("ComponentState") {
        return Some(create_decoded_value_by_type(
            None,
            debug_name,
            DecodedValueType::None,
        ));
    }

    let mut decoded_struct_values = HashMap::with_capacity(generic_args.len());

    for (i, arg) in generic_args.iter().enumerate() {
        if let GenericArg::Type(concrete_type_id) = arg {
            if let Some(decoded_value) = decode_internal_datas(
                values,
                concrete_type_id,
                type_declaration_map,
                type_sizes,
                relocated_memory,
                data_index,
            ) {
                decoded_struct_values.insert(i, decoded_value);
            }
        }
    }

    (!decoded_struct_values.is_empty()).then(|| {
        create_decoded_value_by_type(
            None,
            debug_name,
            DecodedValueType::Struct(decoded_struct_values),
        )
    })
}

fn decode_array(
    generic_args: &[GenericArg],
    debug_name: &str,
    relocated_memory: &[Option<Felt>],
    values: &[Felt],
    data_index: &mut usize,
    type_declaration_map: &HashMap<ConcreteTypeId, TypeDeclaration>,
    type_sizes: &TypeSizeMap,
) -> Option<DecodedValue> {
    if is_valid_relocated_memory_range(values, *data_index, relocated_memory.len()) {
        let current_index: usize = *data_index;

        let (extracted_length, memory_values) =
            extract_memory_values(relocated_memory, values, data_index);

        if memory_values.is_empty() {
            return None;
        }
        *data_index = current_index + 2;
        let decoded_array_values = decode_array_elements_from_memory(
            generic_args,
            extracted_length,
            &memory_values,
            relocated_memory,
            type_declaration_map,
            type_sizes,
        );

        let decoded_value = create_decoded_value_by_type(
            None,
            debug_name,
            DecodedValueType::Array(decoded_array_values),
        );

        Some(decoded_value)
    } else {
        None
    }
}

fn decode_snapshot(
    generic_args: &[GenericArg],
    values: &[Felt],
    data_index: &mut usize,
    relocated_memory: &[Option<Felt>],
    type_declaration_map: &HashMap<ConcreteTypeId, TypeDeclaration>,
    type_sizes: &TypeSizeMap,
) -> Option<DecodedValue> {
    if let Some(GenericArg::Type(inner_type_id)) = generic_args.first() {
        decode_internal_datas(
            values,
            inner_type_id,
            type_declaration_map,
            type_sizes,
            relocated_memory,
            data_index,
        )
    } else {
        None
    }
}

fn decode_array_elements_from_memory(
    generic_args: &[GenericArg],
    array_length: usize,
    memory_values: &[Felt],
    relocated_memory: &[Option<Felt>],
    type_declaration_map: &HashMap<ConcreteTypeId, TypeDeclaration>,
    type_sizes: &TypeSizeMap,
) -> Vec<DecodedValueType> {
    let mut decoded_array_values = Vec::with_capacity(array_length);
    if let Some(GenericArg::Type(concrete_type_id)) = generic_args.first() {
        let mut index: usize = 0;
        for _ in 0..array_length {
            let values_slice = &memory_values[index..];
            let mut local_data_index = 0;

            if let Some(decoded_value) = decode_internal_datas(
                values_slice,
                concrete_type_id,
                type_declaration_map,
                type_sizes,
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

#[inline(always)]
fn extract_memory_values(
    relocated_memory: &[Option<Felt>],
    values: &[Felt],
    value_index: &usize,
) -> (usize, Vec<Felt>) {
    let (start_index, end_index) = (
        values[*value_index].to_usize().unwrap(),
        values[*value_index + 1].to_usize().unwrap(),
    );

    if end_index > start_index {
        let size = end_index - start_index;
        let memory_values: Vec<Felt> = (start_index..end_index)
            .filter_map(|index| relocated_memory.get(index).and_then(|&v| v))
            .collect();

        return (size, memory_values);
    }
    if end_index == start_index {
        return (
            1,
            vec![relocated_memory
                .get(end_index)
                .and_then(|&v| v)
                .unwrap_or(Felt::ZERO)],
        );
    }

    (0, vec![])
}

#[inline(always)]
fn is_valid_relocated_memory_range(
    values: &[Felt],
    data_index: usize,
    relocated_memory_len: usize,
) -> bool {
    if let Some(slice) = values.get(data_index..data_index + 2) {
        if let (Some(start_index), Some(end_index)) = (slice[0].to_usize(), slice[1].to_usize()) {
            return start_index > 0 && end_index >= start_index && end_index < relocated_memory_len;
        }
    }
    false
}
