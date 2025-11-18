use std::borrow::Cow;
use std::collections::{HashMap, HashSet};
use walnut_shared::abi::{Enum, EnumVariant, Function, Input, Output, Struct, StructMember};
use walnut_shared::utils::simplify_type_name;

/// Maximum depth for type expansion to prevent stack overflow with deeply nested types
const MAX_EXPANSION_DEPTH: usize = 5;

/// Type decoder for ABI entrypoints that recursively expands all nested types
pub struct TypeDecoder {
    struct_map: HashMap<String, Struct>,
    enum_map: HashMap<String, Enum>,
}

impl TypeDecoder {
    /// Creates a new TypeDecoder with the given struct and enum maps
    pub fn new(struct_map: HashMap<String, Struct>, enum_map: HashMap<String, Enum>) -> Self {
        Self {
            struct_map,
            enum_map,
        }
    }

    /// Decodes a list of functions with complete type information
    pub fn decode_functions(&self, functions: &[Function]) -> Vec<(String, Function)> {
        functions
            .iter()
            .map(|func| {
                let selector = self.calculate_selector(&func.name);

                // Share visited set across all inputs and outputs of the same function
                // to properly detect circular references
                let mut visited = HashSet::new();

                // Decode inputs with complete type information
                let decoded_inputs = func
                    .inputs
                    .iter()
                    .map(|input| {
                        let enhanced_type = self.enhance_type_recursively(&input.ty);
                        let simplified_type = simplify_type_name(&enhanced_type);

                        // Recursively get all nested type information
                        let (members, variants) =
                            self.get_complete_type_info(&simplified_type, &mut visited, 0);

                        Input {
                            name: input.name.clone(),
                            ty: enhanced_type,
                            struct_members: members.map(|cow| Cow::Owned(cow.into_owned())),
                            enum_variants: variants.map(|cow| Cow::Owned(cow.into_owned())),
                        }
                    })
                    .collect();

                // Decode outputs with complete type information
                let decoded_outputs = func
                    .outputs
                    .iter()
                    .map(|output| {
                        let enhanced_type = self.enhance_type_recursively(&output.ty);
                        let simplified_type = simplify_type_name(&enhanced_type);

                        // Recursively get all nested type information
                        let (members, variants) =
                            self.get_complete_type_info(&simplified_type, &mut visited, 0);

                        Output {
                            ty: enhanced_type,
                            struct_members: members.map(|cow| Cow::Owned(cow.into_owned())),
                            enum_variants: variants.map(|cow| Cow::Owned(cow.into_owned())),
                        }
                    })
                    .collect();

                let decoded_func = Function {
                    name: func.name.clone(),
                    inputs: decoded_inputs,
                    outputs: decoded_outputs,
                    state_mutability: func.state_mutability.clone(),
                };

                (selector, decoded_func)
            })
            .collect()
    }

    /// Recursively enhances a type by expanding all nested types to their deepest level
    fn enhance_type_recursively(&self, type_name: &str) -> String {
        let simplified_type = simplify_type_name(type_name);

        // Handle array types: Array<T> -> recursively enhance T
        if simplified_type.starts_with("Array<") && simplified_type.ends_with(">") {
            let inner_type = &simplified_type[6..simplified_type.len() - 1]; // Remove "Array<" and ">"
            let enhanced_inner = self.enhance_type_recursively(inner_type);
            return format!("Array<{}>", enhanced_inner);
        }

        // Handle Span types: Span<T> -> recursively enhance T (but keep Span name)
        if simplified_type.starts_with("Span<") && simplified_type.ends_with(">") {
            let inner_type = &simplified_type[5..simplified_type.len() - 1]; // Remove "Span<" and ">"
            let enhanced_inner = self.enhance_type_recursively(inner_type);
            return format!("Span<{}>", enhanced_inner);
        }

        // Handle option types: Option<T> -> recursively enhance T
        if simplified_type.starts_with("Option<") && simplified_type.ends_with(">") {
            let inner_type = &simplified_type[7..simplified_type.len() - 1]; // Remove "Option<" and ">"
            let enhanced_inner = self.enhance_type_recursively(inner_type);
            return format!("Option<{}>", enhanced_inner);
        }

        // Handle tuple types: (T1, T2, ...) -> recursively enhance each component
        if simplified_type.starts_with('(') && simplified_type.ends_with(')') {
            let inner_content = &simplified_type[1..simplified_type.len() - 1]; // Remove parentheses
            let components: Vec<&str> = inner_content.split(',').collect();
            let enhanced_components: Vec<String> = components
                .iter()
                .map(|comp| self.enhance_type_recursively(comp.trim()))
                .collect();
            return format!("({})", enhanced_components.join(", "));
        }

        // Handle generic types: GenericType<T1, T2, ...> -> recursively enhance each type parameter
        if simplified_type.contains('<') && simplified_type.contains('>') {
            let generic_name_end = simplified_type.find('<').unwrap();
            let generic_name = &simplified_type[..generic_name_end];
            let type_params_start = generic_name_end + 1;
            let type_params_end = simplified_type.rfind('>').unwrap();
            let type_params_content = &simplified_type[type_params_start..type_params_end];

            // Split type parameters by comma, handling nested generics
            let enhanced_params = self.split_and_enhance_generic_params(type_params_content);

            return format!("{}<{}>", generic_name, enhanced_params.join(", "));
        }

        // For simple types, check if they need expansion
        if self.struct_map.contains_key(&simplified_type) {
            simplified_type
        } else if self.enum_map.contains_key(&simplified_type) {
            simplified_type
        } else {
            simplified_type
        }
    }

    /// Splits generic type parameters and recursively enhances each one
    fn split_and_enhance_generic_params(&self, params_content: &str) -> Vec<String> {
        let mut enhanced_params = Vec::new();
        let mut current_param = String::new();
        let mut bracket_count = 0;

        for ch in params_content.chars() {
            match ch {
                '<' => {
                    bracket_count += 1;
                    current_param.push(ch);
                }
                '>' => {
                    bracket_count -= 1;
                    current_param.push(ch);
                }
                ',' if bracket_count == 0 => {
                    // Only split on comma if we're not inside nested brackets
                    let trimmed = current_param.trim();
                    if !trimmed.is_empty() {
                        let enhanced = self.enhance_type_recursively(trimmed);
                        enhanced_params.push(enhanced);
                    }
                    current_param.clear();
                }
                _ => {
                    current_param.push(ch);
                }
            }
        }

        let trimmed = current_param.trim();
        if !trimmed.is_empty() {
            let enhanced = self.enhance_type_recursively(trimmed);
            enhanced_params.push(enhanced);
        }

        enhanced_params
    }

    /// Recursively extracts complete type information including all nested types
    fn get_complete_type_info(
        &self,
        type_name: &str,
        visited: &mut HashSet<String>,
        depth: usize,
    ) -> (
        Option<Cow<'static, [StructMember]>>,
        Option<Cow<'static, [EnumVariant]>>,
    ) {
        if self.is_primitive_type(type_name) {
            return (None, None);
        }

        // Check for depth limit
        if depth > MAX_EXPANSION_DEPTH {
            return (None, None);
        }

        // Check for circular reference
        if visited.contains(type_name) {
            // Circular reference detected - return empty to break the cycle
            return (None, None);
        }

        visited.insert(type_name.to_string());

        if let Some(struct_def) = self.struct_map.get(type_name) {
            // Special handling for Span<T> types that exist as structs in ABI
            // These are the actual Span<T> struct definitions, not field types
            if type_name.starts_with("Span<") && type_name.ends_with(">") {
                // For Span<T> structs, we need to decode them specially
                // by returning the inner type structure directly
                let inner_type = &type_name[5..type_name.len() - 1];
                let simplified_inner = simplify_type_name(inner_type);

                // Get the structure of the inner type directly
                let (inner_members, inner_variants) =
                    self.get_complete_type_info(&simplified_inner, visited, depth + 1);

                // Return the inner type structure directly instead of wrapping in element
                // Don't remove from visited - keep it for circular reference detection
                return (inner_members, inner_variants);
            }

            // Special handling for u256 and u512 structs - display as primitive types
            if self.is_multi_limb_integer_struct(struct_def) {
                // Return None for struct_members to treat as primitive type
                // Don't remove from visited - keep it for circular reference detection
                return (None, None);
            }

            let enhanced_members = struct_def
                .members
                .iter()
                .map(|member| {
                    let member_type = simplify_type_name(&member.ty);

                    // Check for circular reference or depth limit before recursing
                    let is_circular = visited.contains(&member_type);
                    let exceeds_depth = depth + 1 > MAX_EXPANSION_DEPTH;

                    if is_circular || exceeds_depth {
                        // Circular reference or depth limit reached - return with reference info
                        StructMember {
                            name: member.name.clone(),
                            ty: member.ty.clone(),
                            struct_members: None,
                            enum_variants: None,
                            circular_reference_to: Some(member_type.clone()),
                        }
                    } else {
                        let (nested_members, nested_variants) =
                            self.get_complete_type_info(&member_type, visited, depth + 1);

                        StructMember {
                            name: member.name.clone(),
                            ty: member.ty.clone(),
                            struct_members: nested_members.map(|cow| Cow::Owned(cow.into_owned())),
                            enum_variants: nested_variants.map(|cow| Cow::Owned(cow.into_owned())),
                            circular_reference_to: None,
                        }
                    }
                })
                .collect::<Vec<_>>();

            // Don't remove from visited - keep it for circular reference detection
            return (Some(Cow::Owned(enhanced_members)), None);
        }

        if let Some(enum_def) = self.enum_map.get(type_name) {
            let enhanced_variants = enum_def
                .variants
                .iter()
                .map(|variant| {
                    let variant_type = simplify_type_name(&variant.ty);

                    // Check for circular reference or depth limit before recursing
                    let is_circular = visited.contains(&variant_type);
                    let exceeds_depth = depth + 1 > MAX_EXPANSION_DEPTH;

                    if is_circular || exceeds_depth {
                        // Circular reference or depth limit reached - return with reference info
                        EnumVariant {
                            name: variant.name.clone(),
                            ty: variant.ty.clone(),
                            struct_members: None,
                            enum_variants: None,
                            circular_reference_to: Some(variant_type.clone()),
                        }
                    } else {
                        let (nested_members, nested_variants) =
                            self.get_complete_type_info(&variant_type, visited, depth + 1);

                        EnumVariant {
                            name: variant.name.clone(),
                            ty: variant.ty.clone(),
                            struct_members: nested_members.map(|cow| Cow::Owned(cow.into_owned())),
                            enum_variants: nested_variants.map(|cow| Cow::Owned(cow.into_owned())),
                            circular_reference_to: None,
                        }
                    }
                })
                .collect::<Vec<_>>();

            // Don't remove from visited - keep it for circular reference detection
            return (None, Some(Cow::Owned(enhanced_variants)));
        }

        // Check if this is an array type that needs recursive expansion
        if type_name.starts_with("Array<") && type_name.ends_with(">") {
            let inner_type = &type_name[6..type_name.len() - 1];
            let simplified_inner = simplify_type_name(inner_type);
            let result = self.get_complete_type_info(&simplified_inner, visited, depth + 1);
            // Don't remove from visited - keep it for circular reference detection
            return result;
        }

        // Check if this is a Span type that needs recursive expansion (treat as Array)
        if type_name.starts_with("Span<") && type_name.ends_with(">") {
            let inner_type = &type_name[5..type_name.len() - 1];
            let simplified_inner = simplify_type_name(inner_type);

            // For Span types, we need to check if the inner type has structure
            let (inner_members, inner_variants) =
                self.get_complete_type_info(&simplified_inner, visited, depth + 1);

            // Only create members if the inner type actually has structure
            if inner_members.is_some() || inner_variants.is_some() {
                let synthetic_member = StructMember {
                    name: "element".to_string(),
                    ty: inner_type.to_string(),
                    struct_members: inner_members,
                    enum_variants: inner_variants,
                    circular_reference_to: None,
                };

                // Don't remove from visited - keep it for circular reference detection
                return (Some(Cow::Owned(vec![synthetic_member])), None);
            }

            // Don't remove from visited - keep it for circular reference detection
            return (None, None);
        }

        // Check if this is an option type that needs recursive expansion
        if type_name.starts_with("Option<") && type_name.ends_with(">") {
            let inner_type = &type_name[7..type_name.len() - 1];
            let simplified_inner = simplify_type_name(inner_type);
            let result = self.get_complete_type_info(&simplified_inner, visited, depth + 1);
            // Don't remove from visited - keep it for circular reference detection
            return result;
        }

        // Check if this is a tuple type that needs recursive expansion
        if type_name.starts_with('(') && type_name.ends_with(')') {
            // For tuples, we need to collect all nested type information
            let inner_content = &type_name[1..type_name.len() - 1];
            let components: Vec<&str> = inner_content.split(',').collect();

            // Create a synthetic struct member for each tuple component
            let mut tuple_members = Vec::new();
            for (index, component) in components.iter().enumerate() {
                let simplified_comp = simplify_type_name(component.trim());
                let (members, variants) =
                    self.get_complete_type_info(&simplified_comp, visited, depth + 1);

                // Create a synthetic member for this tuple component
                let member_name = format!("tuple_{}", index);
                let member = StructMember {
                    name: member_name,
                    ty: component.trim().to_string(),
                    struct_members: members.map(|cow| Cow::Owned(cow.into_owned())),
                    enum_variants: variants.map(|cow| Cow::Owned(cow.into_owned())),
                    circular_reference_to: None,
                };
                tuple_members.push(member);
            }

            if !tuple_members.is_empty() {
                // Don't remove from visited - keep it for circular reference detection
                return (Some(Cow::Owned(tuple_members)), None);
            }
        }

        // Check if this is a generic type that needs recursive expansion
        if type_name.contains('<') && type_name.contains('>') {
            let generic_name_end = type_name.find('<').unwrap();
            let type_params_start = generic_name_end + 1;
            let type_params_end = type_name.rfind('>').unwrap();
            let type_params_content = &type_name[type_params_start..type_params_end];

            // Try to find the first type parameter that has structure
            let params = self.split_generic_params(type_params_content);
            for param in params {
                let simplified_param = simplify_type_name(&param);
                let (members, variants) =
                    self.get_complete_type_info(&simplified_param, visited, depth + 1);
                if members.is_some() || variants.is_some() {
                    // Don't remove from visited - keep it for circular reference detection
                    return (members, variants);
                }
            }
        }

        // No nested structure found
        // Don't remove from visited - keep it for circular reference detection
        (None, None)
    }

    /// Splits generic type parameters without recursive enhancement (used for type info extraction)
    fn split_generic_params(&self, params_content: &str) -> Vec<String> {
        let mut params = Vec::new();
        let mut current_param = String::new();
        let mut bracket_count = 0;

        for ch in params_content.chars() {
            match ch {
                '<' => {
                    bracket_count += 1;
                    current_param.push(ch);
                }
                '>' => {
                    bracket_count -= 1;
                    current_param.push(ch);
                }
                ',' if bracket_count == 0 => {
                    // Only split on comma if we're not inside nested brackets
                    let trimmed = current_param.trim();
                    if !trimmed.is_empty() {
                        params.push(trimmed.to_string());
                    }
                    current_param.clear();
                }
                _ => {
                    current_param.push(ch);
                }
            }
        }

        let trimmed = current_param.trim();
        if !trimmed.is_empty() {
            params.push(trimmed.to_string());
        }

        params
    }

    /// Check if a type is a primitive type that doesn't need expansion
    fn is_primitive_type(&self, type_name: &str) -> bool {
        let primitive_types = [
            "felt252",
            "felt",
            "u8",
            "u16",
            "u32",
            "u64",
            "u128",
            "u256",
            "u512",
            "usize",
            "i8",
            "i16",
            "i32",
            "i64",
            "i128",
            "bool",
            "ContractAddress",
            "EthAddress",
            "ClassHash",
            "bytes31",
            "Panic",
        ];

        primitive_types.contains(&type_name)
    }

    /// Check if a struct represents a multi-limb integer (u256 or u512)
    fn is_multi_limb_integer_struct(&self, struct_def: &Struct) -> bool {
        // Check for u256 struct pattern: exactly 2 members with names "low" and "high" of type u128
        if struct_def.members.len() == 2 {
            let has_low = struct_def
                .members
                .iter()
                .any(|m| m.name == "low" && m.ty == "u128");
            let has_high = struct_def
                .members
                .iter()
                .any(|m| m.name == "high" && m.ty == "u128");
            if has_low && has_high {
                return true;
            }
        }

        // Check for u512 struct pattern: exactly 4 members with names "limb0", "limb1", "limb2", "limb3" of type u128
        if struct_def.members.len() == 4 {
            let has_limb0 = struct_def
                .members
                .iter()
                .any(|m| m.name == "limb0" && m.ty == "u128");
            let has_limb1 = struct_def
                .members
                .iter()
                .any(|m| m.name == "limb1" && m.ty == "u128");
            let has_limb2 = struct_def
                .members
                .iter()
                .any(|m| m.name == "limb2" && m.ty == "u128");
            let has_limb3 = struct_def
                .members
                .iter()
                .any(|m| m.name == "limb3" && m.ty == "u128");
            if has_limb0 && has_limb1 && has_limb2 && has_limb3 {
                return true;
            }
        }

        false
    }

    /// Calculate selector for a function name
    fn calculate_selector(&self, function_name: &str) -> String {
        use starknet_api::abi::abi_utils::selector_from_name;
        selector_from_name(function_name).0.to_hex_string()
    }
}

/// Recursively expands all structs with their nested members
pub fn expand_structs_recursively(
    struct_map: &HashMap<String, Struct>,
    enum_map: &HashMap<String, Enum>,
) -> HashMap<String, Struct> {
    let mut expanded = HashMap::new();
    let mut visited = HashSet::new();

    for name in struct_map.keys() {
        if !visited.contains(name) {
            expand_struct_recursively(name, struct_map, enum_map, &mut expanded, &mut visited);
        }
    }

    expanded
}

/// Recursively expands all enums with their nested variants
pub fn expand_enums_recursively(
    enum_map: &HashMap<String, Enum>,
    struct_map: &HashMap<String, Struct>,
) -> HashMap<String, Enum> {
    let mut expanded = HashMap::new();
    let mut visited = HashSet::new();

    for name in enum_map.keys() {
        if !visited.contains(name) {
            expand_enum_recursively(name, enum_map, struct_map, &mut expanded, &mut visited);
        }
    }

    expanded
}

fn expand_struct_recursively(
    struct_name: &str,
    struct_map: &HashMap<String, Struct>,
    enum_map: &HashMap<String, Enum>,
    expanded: &mut HashMap<String, Struct>,
    visited: &mut HashSet<String>,
) -> Struct {
    if let Some(cached) = expanded.get(struct_name) {
        return cached.clone();
    }

    visited.insert(struct_name.to_string());

    let struct_def = match struct_map.get(struct_name) {
        Some(def) => def,
        None => {
            return Struct {
                name: struct_name.to_string(),
                members: vec![],
            }
        }
    };

    let expanded_members = struct_def
        .members
        .iter()
        .map(|member| {
            let member_type = simplify_type_name(&member.ty);

            if struct_map.contains_key(&member_type) {
                // Check for circular reference to prevent infinite recursion
                if visited.contains(&member_type) {
                    // Circular reference detected - return basic member info without expansion
                    StructMember {
                        name: member.name.clone(),
                        ty: member_type.clone(),
                        struct_members: None,
                        enum_variants: None,
                        circular_reference_to: Some(member_type.clone()),
                    }
                } else {
                    let expanded_nested = expand_struct_recursively(
                        &member_type,
                        struct_map,
                        enum_map,
                        expanded,
                        visited,
                    );

                    StructMember {
                        name: member.name.clone(),
                        ty: member_type.clone(),
                        struct_members: Some(Cow::Owned(expanded_nested.members.clone())),
                        enum_variants: None,
                        circular_reference_to: None,
                    }
                }
            } else if enum_map.contains_key(&member_type) {
                let enum_def = enum_map.get(&member_type).unwrap();
                StructMember {
                    name: member.name.clone(),
                    ty: member_type.clone(),
                    struct_members: None,
                    enum_variants: Some(Cow::Owned(enum_def.variants.clone())),
                    circular_reference_to: None,
                }
            } else {
                StructMember {
                    name: member.name.clone(),
                    ty: member_type.clone(),
                    struct_members: None,
                    enum_variants: None,
                    circular_reference_to: None,
                }
            }
        })
        .collect();

    let expanded_struct = Struct {
        name: struct_name.to_string(),
        members: expanded_members,
    };

    expanded.insert(struct_name.to_string(), expanded_struct.clone());
    expanded_struct
}

fn expand_enum_recursively(
    enum_name: &str,
    enum_map: &HashMap<String, Enum>,
    struct_map: &HashMap<String, Struct>,
    expanded: &mut HashMap<String, Enum>,
    visited: &mut HashSet<String>,
) -> Enum {
    // Return cached result if available
    if let Some(cached) = expanded.get(enum_name) {
        return cached.clone();
    }

    visited.insert(enum_name.to_string());

    let enum_def = match enum_map.get(enum_name) {
        Some(def) => def,
        None => {
            return Enum {
                name: enum_name.to_string(),
                variants: vec![],
            }
        }
    };

    let expanded_variants = enum_def
        .variants
        .iter()
        .map(|variant| {
            let variant_type = simplify_type_name(&variant.ty);

            // Check if this variant is a struct that we can expand
            if struct_map.contains_key(&variant_type) {
                if let Some(struct_def) = struct_map.get(&variant_type) {
                    EnumVariant {
                        name: variant.name.clone(),
                        ty: variant_type.clone(),
                        struct_members: Some(Cow::Owned(struct_def.members.clone())),
                        enum_variants: None,
                        circular_reference_to: None,
                    }
                } else {
                    // Fallback if struct not found
                    EnumVariant {
                        name: variant.name.clone(),
                        ty: variant_type.clone(),
                        struct_members: None,
                        enum_variants: None,
                        circular_reference_to: None,
                    }
                }
            }
            // Check if this variant is another enum (nested enum)
            else if enum_map.contains_key(&variant_type) && variant_type != enum_name {
                // Check for circular reference to prevent infinite recursion
                if visited.contains(&variant_type) {
                    // Return basic variant info without expansion to avoid cycles
                    EnumVariant {
                        name: variant.name.clone(),
                        ty: variant_type.clone(),
                        struct_members: None,
                        enum_variants: None,
                        circular_reference_to: Some(variant_type.clone()),
                    }
                } else {
                    let nested_enum = expand_enum_recursively(
                        &variant_type,
                        enum_map,
                        struct_map,
                        expanded,
                        visited,
                    );

                    EnumVariant {
                        name: variant.name.clone(),
                        ty: variant_type.clone(),
                        struct_members: None,
                        enum_variants: Some(Cow::Owned(nested_enum.variants.clone())),
                        circular_reference_to: None,
                    }
                }
            } else {
                // Primitive type or unknown type
                EnumVariant {
                    name: variant.name.clone(),
                    ty: variant_type.clone(),
                    struct_members: None,
                    enum_variants: None,
                    circular_reference_to: None,
                }
            }
        })
        .collect();

    let expanded_enum = Enum {
        name: enum_name.to_string(),
        variants: expanded_variants,
    };

    expanded.insert(enum_name.to_string(), expanded_enum.clone());
    expanded_enum
}

#[cfg(test)]
mod tests {
    use super::*;
    use walnut_shared::abi::{Enum, EnumVariant, Function, Input, Output, Struct, StructMember};

    /// Test circular reference detection in struct expansion
    /// This simulates the Layout -> FieldLayout -> Layout circular reference
    #[test]
    fn test_circular_reference_struct_expansion() {
        // Create a circular reference: StructA -> StructB -> StructA
        let mut struct_map = HashMap::new();
        let mut enum_map = HashMap::new();

        // StructA contains StructB
        struct_map.insert(
            "StructA".to_string(),
            Struct {
                name: "StructA".to_string(),
                members: vec![StructMember {
                    name: "b".to_string(),
                    ty: "StructB".to_string(),
                    struct_members: None,
                    enum_variants: None,
                    circular_reference_to: None,
                }],
            },
        );

        // StructB contains StructA (circular reference)
        struct_map.insert(
            "StructB".to_string(),
            Struct {
                name: "StructB".to_string(),
                members: vec![StructMember {
                    name: "a".to_string(),
                    ty: "StructA".to_string(),
                    struct_members: None,
                    enum_variants: None,
                    circular_reference_to: None,
                }],
            },
        );

        // Expand structs - should not cause infinite recursion
        let expanded = expand_structs_recursively(&struct_map, &enum_map);

        // Should successfully expand both structs
        assert!(expanded.contains_key("StructA"));
        assert!(expanded.contains_key("StructB"));

        // StructA should have StructB as member, but StructB should not be expanded
        // (to break the cycle)
        let struct_a = expanded.get("StructA").unwrap();
        assert_eq!(struct_a.members.len(), 1);
        assert_eq!(struct_a.members[0].name, "b");
        // The nested struct should be None to break the cycle
        assert!(struct_a.members[0].struct_members.is_none());
    }

    /// Test circular reference detection in enum expansion
    #[test]
    fn test_circular_reference_enum_expansion() {
        let mut struct_map = HashMap::new();
        let mut enum_map = HashMap::new();

        // EnumA contains EnumB
        enum_map.insert(
            "EnumA".to_string(),
            Enum {
                name: "EnumA".to_string(),
                variants: vec![EnumVariant {
                    name: "VariantB".to_string(),
                    ty: "EnumB".to_string(),
                    struct_members: None,
                    enum_variants: None,
                    circular_reference_to: None,
                }],
            },
        );

        // EnumB contains EnumA (circular reference)
        enum_map.insert(
            "EnumB".to_string(),
            Enum {
                name: "EnumB".to_string(),
                variants: vec![EnumVariant {
                    name: "VariantA".to_string(),
                    ty: "EnumA".to_string(),
                    struct_members: None,
                    enum_variants: None,
                    circular_reference_to: None,
                }],
            },
        );

        // Expand enums - should not cause infinite recursion
        let expanded = expand_enums_recursively(&enum_map, &struct_map);

        // Should successfully expand both enums
        assert!(expanded.contains_key("EnumA"));
        assert!(expanded.contains_key("EnumB"));

        // EnumA should have EnumB as variant, but EnumB should not be expanded
        // (to break the cycle)
        let enum_a = expanded.get("EnumA").unwrap();
        assert_eq!(enum_a.variants.len(), 1);
        assert_eq!(enum_a.variants[0].name, "VariantB");
        // The nested enum should be None to break the cycle
        assert!(enum_a.variants[0].enum_variants.is_none());
    }

    /// Test circular reference detection in get_complete_type_info
    /// This simulates the Layout -> FieldLayout -> Layout case from the real bug
    #[test]
    fn test_circular_reference_get_complete_type_info() {
        let mut struct_map = HashMap::new();
        let mut enum_map = HashMap::new();

        // Create Layout enum with Struct variant containing FieldLayout
        enum_map.insert(
            "Layout".to_string(),
            Enum {
                name: "Layout".to_string(),
                variants: vec![EnumVariant {
                    name: "Struct".to_string(),
                    ty: "Span<FieldLayout>".to_string(),
                    struct_members: None,
                    enum_variants: None,
                    circular_reference_to: None,
                }],
            },
        );

        // Create FieldLayout struct containing Layout (circular reference)
        struct_map.insert(
            "FieldLayout".to_string(),
            Struct {
                name: "FieldLayout".to_string(),
                members: vec![
                    StructMember {
                        name: "selector".to_string(),
                        ty: "felt252".to_string(),
                        struct_members: None,
                        enum_variants: None,
                        circular_reference_to: None,
                    },
                    StructMember {
                        name: "layout".to_string(),
                        ty: "Layout".to_string(), // Circular reference!
                        struct_members: None,
                        enum_variants: None,
                        circular_reference_to: None,
                    },
                ],
            },
        );

        // Create TypeDecoder
        let decoder = TypeDecoder::new(struct_map, enum_map);

        // Create a function that uses Layout type
        let function = Function {
            name: "test_function".to_string(),
            inputs: vec![Input {
                name: "layout".to_string(),
                ty: "Layout".to_string(),
                struct_members: None,
                enum_variants: None,
            }],
            outputs: vec![],
            state_mutability: walnut_shared::abi::StateMutability::View,
        };

        // Decode function - should not cause infinite recursion
        let decoded = decoder.decode_functions(&[function]);

        // Should successfully decode
        assert_eq!(decoded.len(), 1);
        let (_, decoded_func) = &decoded[0];
        assert_eq!(decoded_func.inputs.len(), 1);

        // The Layout enum should be expanded, but when it encounters FieldLayout
        // which contains Layout again, it should detect the circular reference
        // and return None for nested types to break the cycle
        let input = &decoded_func.inputs[0];
        assert!(input.enum_variants.is_some());
        let variants = input.enum_variants.as_ref().unwrap();
        assert_eq!(variants.len(), 1);
        assert_eq!(variants[0].name, "Struct");

        // The Struct variant should have FieldLayout members, but when FieldLayout
        // tries to expand Layout again, it should detect the cycle and return None
        // This is the key fix - it prevents infinite recursion
    }

    /// Test that visited set is shared across inputs and outputs
    #[test]
    fn test_visited_set_shared_across_function() {
        let mut struct_map = HashMap::new();
        let mut enum_map = HashMap::new();

        // Create a struct that references itself
        struct_map.insert(
            "SelfRefStruct".to_string(),
            Struct {
                name: "SelfRefStruct".to_string(),
                members: vec![StructMember {
                    name: "self_ref".to_string(),
                    ty: "SelfRefStruct".to_string(),
                    struct_members: None,
                    enum_variants: None,
                    circular_reference_to: None,
                }],
            },
        );

        let decoder = TypeDecoder::new(struct_map, enum_map);

        // Create a function with both input and output using the same self-referential type
        let function = Function {
            name: "test_function".to_string(),
            inputs: vec![Input {
                name: "input".to_string(),
                ty: "SelfRefStruct".to_string(),
                struct_members: None,
                enum_variants: None,
            }],
            outputs: vec![Output {
                ty: "SelfRefStruct".to_string(),
                struct_members: None,
                enum_variants: None,
            }],
            state_mutability: walnut_shared::abi::StateMutability::View,
        };

        // Decode function - should not cause infinite recursion
        // The visited set should be shared, so when processing the output,
        // it should detect that SelfRefStruct was already visited in the input
        let decoded = decoder.decode_functions(&[function]);

        assert_eq!(decoded.len(), 1);
        // Both input and output should be successfully decoded without stack overflow
        assert_eq!(decoded[0].1.inputs.len(), 1);
        assert_eq!(decoded[0].1.outputs.len(), 1);
    }
}
