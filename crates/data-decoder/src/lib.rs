pub mod calldata_decoder;
mod common;
pub mod internal_function_decoder;
mod starknet_types;
use common::SKIP_BUILTIN_TYPES;

pub fn skip_builtin_type_declaration(type_name: &str) -> bool {
    SKIP_BUILTIN_TYPES
        .iter()
        .any(|&builtin| type_name.starts_with(builtin))
}

pub fn simplify_type_name(type_str: &str) -> String {
    let type_str = if (type_str.starts_with("Tuple<")
        || type_str.starts_with("core::panics::PanicResult::<"))
        && type_str.ends_with(">")
    {
        if let Some(start) = type_str.find('<') {
            let inner_type_str = &type_str[start + 1..type_str.len() - 1];
            if inner_type_str.starts_with("(") && inner_type_str.ends_with(")") {
                &inner_type_str[1..inner_type_str.len() - 1]
            } else {
                inner_type_str
            }
        } else {
            type_str
        }
    } else {
        type_str
    };

    let types: Vec<&str> = type_str.split(", ").collect();

    let parsed_types: Vec<String> = types
        .iter()
        .map(|&t| {
            if let Some(first_angle_bracket) = t.find('<') {
                let main_type = &t[..first_angle_bracket]; // Before first '<'
                let inner_type = &t[first_angle_bracket + 1..t.len() - 1]; // Inside '<>'
                let main_type_name = main_type
                    .rsplit("::")
                    .find(|&part| !part.is_empty())
                    .unwrap_or(main_type);

                let parsed_inner_type = simplify_type_name(inner_type); // Nested inner type
                format!("{}<{}>", main_type_name, parsed_inner_type)
            } else {
                t.rsplit("::")
                    .find(|&part| !part.is_empty())
                    .unwrap_or(t)
                    .to_string()
            }
        })
        .collect();
    if !parsed_types.is_empty() {
        if parsed_types.len() > 1 {
            return format!("({})", parsed_types.join(", "));
        }
        return parsed_types.first().unwrap().clone();
    }

    String::new()
}
