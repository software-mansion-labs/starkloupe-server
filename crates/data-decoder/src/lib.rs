pub mod calldata_decoder;
mod common;
pub mod internal_function_decoder;
mod starknet_types;
use common::SKIP_BUILTIN_TYPES;
use fancy_regex::Regex;

pub fn skip_builtin_type_declaration(type_name: &str) -> bool {
    SKIP_BUILTIN_TYPES
        .iter()
        .any(|&builtin| type_name.contains(builtin))
}

pub fn simplify_type_name(type_name: &str) -> String {
    // Regex for removing leading namespaces
    let namespace_regex = Regex::new(r"(\w+::)+(?=\w+)").unwrap();
    // Regex for cleaning up generics
    let generic_regex = Regex::new(r"<([^<>]*)>").unwrap();
    // Regex for cleaning up parentheses with trailing commas
    let bracket_spaces_regex = Regex::new(r"^\(\s*(.*?)\s*,*\s*\),*$").unwrap();

    // Step 1: Remove leading namespaces using the namespace regex
    let mut simplified = namespace_regex.replace_all(type_name, "").to_string();

    // Step 2: Process generics by cleaning up inside the <...>
    simplified = generic_regex
        .replace_all(&simplified, |caps: &fancy_regex::Captures| {
            let mut inside = caps.get(1).unwrap().as_str().trim().to_string();

            // Step 3: Handle the edge case of parentheses with trailing commas
            if let Ok(Some(bracket_match)) = bracket_spaces_regex.captures(&inside) {
                inside = bracket_match.get(1).unwrap().as_str().trim().to_string();
            }

            // Step 4: Remove leading namespaces inside generics
            format!("<{}>", inside.replace(r"(\w+::)+", ""))
        })
        .to_string();

    // Step 5: Handle the specific case to remove <, ( from start and end
    let surrounding_regex = Regex::new(r"^<(\(?)(.*?)(\)?)>$").unwrap();
    if let Ok(Some(caps)) = surrounding_regex.captures(&simplified) {
        return caps.get(2).unwrap().as_str().trim().to_string();
    }

    simplified.trim().to_string()
}
