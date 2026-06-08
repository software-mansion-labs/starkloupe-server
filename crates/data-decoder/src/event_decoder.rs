use crate::utils::skip_builtin_type_declaration;
use crate::{create_decoded_value_by_type, DecodedValue, DecodedValueType};
use cairo_lang_sierra::ids::ConcreteTypeId;
use cairo_lang_sierra::program::{GenericArg, TypeDeclaration};
use num_traits::cast::ToPrimitive;
use starknet_types_core::felt::Felt;
use std::collections::{BTreeMap, HashMap};
use walnut_shared::utils::simplify_type_name;

pub fn decode_event_datas(
    type_id: &ConcreteTypeId,
    type_declaration_map: &HashMap<ConcreteTypeId, TypeDeclaration>,
    values: &[Felt],
    data_index: &mut usize,
) -> Option<DecodedValue> {
    let type_declaration = type_declaration_map.get(type_id)?;
    let generic_type_id = &type_declaration.long_id.generic_id;
    let generic_args = &type_declaration.long_id.generic_args;
    let debug_name = simplify_type_name(type_declaration.id.debug_name.as_deref().unwrap_or(""));

    if skip_builtin_type_declaration(debug_name.as_str()) {
        return None;
    }

    if values.is_empty() {
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
        ),
        "Struct" => decode_struct(
            values,
            &debug_name,
            generic_args,
            data_index,
            type_declaration_map,
        ),
        "Array" => decode_array(
            generic_args,
            &debug_name,
            values,
            data_index,
            type_declaration_map,
        ),
        "Snapshot" => decode_snapshot(generic_args, values, data_index, type_declaration_map),
        // https://docs.swmansion.com/scarb/corelib/core-zeroable-NonZero.html
        "NonZero" => {
            if let Some(GenericArg::Type(inner_type_id)) = generic_args.first() {
                decode_event_datas(inner_type_id, type_declaration_map, values, data_index)
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
) -> Option<DecodedValue> {
    if let Some(value) = values.get(*data_index) {
        let variant_index = value.to_usize()?;

        let concrete_type_id = match generic_args.get(variant_index + 1) {
            Some(GenericArg::Type(id)) => id,
            _ => return None,
        };

        *data_index += 1;

        if let Some("Unit") = concrete_type_id.debug_name.as_deref() {
            return Some(create_decoded_value_by_type(
                None,
                debug_name,
                DecodedValueType::Single(Felt::from(variant_index)),
            ));
        }

        let decoded_value =
            decode_event_datas(concrete_type_id, type_declaration_map, values, data_index)?;
        Some(create_decoded_value_by_type(
            None,
            debug_name,
            DecodedValueType::Enum(debug_name.to_string(), Box::new(decoded_value)),
        ))
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
) -> Option<DecodedValue> {
    let simplified_debug_name = simplify_type_name(debug_name);
    if simplified_debug_name.starts_with("Span<") {
        decode_span(
            values,
            &simplified_debug_name,
            generic_args,
            data_index,
            type_declaration_map,
        )
    } else if simplified_debug_name.starts_with("(") && simplified_debug_name.ends_with(")") {
        decode_tuple(
            values,
            &simplified_debug_name,
            generic_args,
            data_index,
            type_declaration_map,
        )
    } else {
        decode_standard_struct(
            values,
            &simplified_debug_name,
            generic_args,
            data_index,
            type_declaration_map,
        )
    }
}

fn decode_span(
    values: &[Felt],
    debug_name: &str,
    generic_args: &[GenericArg],
    data_index: &mut usize,
    type_declaration_map: &HashMap<ConcreteTypeId, TypeDeclaration>,
) -> Option<DecodedValue> {
    // Span is a struct with one field, which is the inner array
    // The inner array type is the second generic argument
    if let Some(GenericArg::Type(inner_type_id)) = generic_args.get(1) {
        decode_event_datas(inner_type_id, type_declaration_map, values, data_index).map(
            |decoded_value| create_decoded_value_by_type(None, debug_name, decoded_value.value),
        )
    } else {
        None
    }
}

fn decode_tuple(
    values: &[Felt],
    debug_name: &str,
    generic_args: &[GenericArg],
    data_index: &mut usize,
    type_declaration_map: &HashMap<ConcreteTypeId, TypeDeclaration>,
) -> Option<DecodedValue> {
    let mut tuple_elements = Vec::new();

    for arg in generic_args.iter() {
        if let GenericArg::Type(concrete_type_id) = arg {
            if let Some(decoded_value) =
                decode_event_datas(concrete_type_id, type_declaration_map, values, data_index)
            {
                tuple_elements.push(decoded_value.value);
            }
        }
    }

    if !tuple_elements.is_empty() {
        Some(create_decoded_value_by_type(
            None,
            debug_name,
            DecodedValueType::Array(tuple_elements),
        ))
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
) -> Option<DecodedValue> {
    let mut decoded_struct_values = BTreeMap::new();

    for (i, arg) in generic_args.iter().enumerate() {
        if let GenericArg::Type(concrete_type_id) = arg {
            if let Some(decoded_value) =
                decode_event_datas(concrete_type_id, type_declaration_map, values, data_index)
            {
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
    values: &[Felt],
    data_index: &mut usize,
    type_declaration_map: &HashMap<ConcreteTypeId, TypeDeclaration>,
) -> Option<DecodedValue> {
    let array_length = values.get(*data_index)?.to_usize()?;
    *data_index += 1;

    let mut decoded_elements = Vec::with_capacity(array_length);

    if let Some(GenericArg::Type(concrete_type_id)) = generic_args.first() {
        for _ in 0..array_length {
            if let Some(decoded_value) =
                decode_event_datas(concrete_type_id, type_declaration_map, values, data_index)
            {
                decoded_elements.push(decoded_value.value);
            } else {
                return None;
            }
        }
    }

    Some(create_decoded_value_by_type(
        None,
        debug_name,
        DecodedValueType::Array(decoded_elements),
    ))
}

fn decode_snapshot(
    generic_args: &[GenericArg],
    values: &[Felt],
    data_index: &mut usize,
    type_declaration_map: &HashMap<ConcreteTypeId, TypeDeclaration>,
) -> Option<DecodedValue> {
    if let Some(GenericArg::Type(inner_type_id)) = generic_args.first() {
        decode_event_datas(inner_type_id, type_declaration_map, values, data_index)
    } else {
        None
    }
}
