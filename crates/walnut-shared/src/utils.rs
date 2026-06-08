use cairo_lang_starknet_classes::compiler_version::VersionId;
use starknet_rust::core::types::ContractClass as ContractClassStarknet;
use starknet_rust::core::types::Felt;

pub fn simplify_type_name(type_str: &str) -> String {
    if let (Some(inner), Some(_)) = (type_str.strip_prefix('['), type_str.strip_suffix(']')) {
        if let Some(delim_index) = inner.find(';') {
            let element_type = inner[..delim_index].trim();
            let length_end = inner.len();
            let length = &inner[delim_index + 1..length_end - 1].trim();
            let simplified_element = element_type
                .rfind("::")
                .map(|pos| &element_type[pos + 2..])
                .unwrap_or(element_type);
            return format!("[{};{}]", simplified_element, length);
        }
    }
    let inner_content = type_str
        .strip_prefix("Tuple<")
        .and_then(|s| s.strip_suffix('>'))
        .or_else(|| type_str.strip_prefix('(').and_then(|s| s.strip_suffix(')')))
        .unwrap_or(type_str);

    let mut parsed_types = Vec::with_capacity(inner_content.matches(", ").count() + 1);

    let mut current = String::new();
    let mut bracket_depth = 0;

    for c in inner_content.chars() {
        match c {
            '<' => {
                bracket_depth += 1;
                current.push(c);
            }
            '>' => {
                bracket_depth -= 1;
                current.push(c);
            }
            ',' if bracket_depth == 0 => {
                parsed_types.push(simplify_single_type(&current));
                current.clear();
            }
            _ => {
                current.push(c);
            }
        }
    }

    if !current.is_empty() {
        parsed_types.push(simplify_single_type(&current));
    }

    let simplified_type = if parsed_types.len() > 1 {
        format!("({})", parsed_types.join(", "))
    } else {
        parsed_types.first().unwrap_or(&String::new()).to_string()
    };

    simplified_type
}

#[inline(always)]
fn simplify_single_type(t: &str) -> String {
    if let Some(first_angle_bracket) = t.find('<') {
        let main_type = &t[..first_angle_bracket];
        let inner_type = &t[first_angle_bracket + 1..t.len() - 1];
        format_generic_type(main_type, inner_type)
    } else {
        extract_last_segment(t).to_string()
    }
}

#[inline(always)]
fn format_generic_type(main_type: &str, inner_type: &str) -> String {
    let main_type_name = extract_last_segment(main_type);
    let parsed_inner_type = simplify_type_name(inner_type);
    if !main_type_name.contains("PanicResult") {
        format!("{}<{}>", main_type_name, parsed_inner_type)
    } else {
        // Remove PanicResult wrapper and return only the inner type
        remove_unwanted_types(&parsed_inner_type)
    }
}

#[inline(always)]
fn extract_last_segment(t: &str) -> &str {
    t.rsplit("::").find(|&part| !part.is_empty()).unwrap_or(t)
}

#[inline(always)]
pub fn remove_unwanted_types(type_str: &str) -> String {
    let is_tuple = type_str.starts_with('(') && type_str.ends_with(')');
    let inner = if is_tuple {
        &type_str[1..type_str.len() - 1]
    } else {
        type_str
    };

    let mut out = String::with_capacity(type_str.len());
    if is_tuple {
        out.push('(');
    }
    let mut first = true;
    for seg in inner.split(',') {
        let seg = seg.trim();
        if seg.is_empty() {
            continue;
        }
        if seg != "()" && (seg.contains("ContractState") || seg.contains("ComponentState")) {
            continue;
        }
        if !first {
            out.push_str(", ");
        }
        out.push_str(seg);
        first = false;
    }
    if is_tuple {
        out.push(')');
    }
    out
}

pub fn convert_contract_class(
    from: ContractClassStarknet,
) -> Option<cairo_lang_starknet_classes::contract_class::ContractClass> {
    match from {
        starknet_rust::core::types::ContractClass::Sierra(ref class) => {
            Some(cairo_lang_starknet_classes::contract_class::ContractClass {
                sierra_program: class
                    .sierra_program
                    .iter()
                    .map(|felt| felt.to_biguint().into())
                    .collect(),
                sierra_program_debug_info: None,
                contract_class_version: class.contract_class_version.clone(),
                entry_points_by_type:
                    cairo_lang_starknet_classes::contract_class::ContractEntryPoints {
                        external: class
                            .entry_points_by_type
                            .external
                            .iter()
                            .map(|entry_point| {
                                cairo_lang_starknet_classes::contract_class::ContractEntryPoint {
                                    selector: entry_point.selector.to_biguint(),
                                    function_idx: entry_point.function_idx as usize,
                                }
                            })
                            .collect(),
                        l1_handler: class
                            .entry_points_by_type
                            .l1_handler
                            .iter()
                            .map(|entry_point| {
                                cairo_lang_starknet_classes::contract_class::ContractEntryPoint {
                                    selector: entry_point.selector.to_biguint(),
                                    function_idx: entry_point.function_idx as usize,
                                }
                            })
                            .collect(),
                        constructor: class
                            .entry_points_by_type
                            .constructor
                            .iter()
                            .map(|entry_point| {
                                cairo_lang_starknet_classes::contract_class::ContractEntryPoint {
                                    selector: entry_point.selector.to_biguint(),
                                    function_idx: entry_point.function_idx as usize,
                                }
                            })
                            .collect(),
                    },
                abi: None,
            })
        }
        _ => None,
    }
}

pub fn extract_sierra_and_cairo_versions(
    sierra_program: &[Felt],
) -> (Option<String>, Option<String>) {
    if sierra_program.len() < 6 {
        return (None, None);
    }
    let sierra_version = format!(
        "{}.{}.{}",
        sierra_program[0], sierra_program[1], sierra_program[2]
    );

    let cairo_version = format!(
        "{}.{}.{}",
        sierra_program[3], sierra_program[4], sierra_program[5]
    );

    (Some(sierra_version), Some(cairo_version))
}

pub fn is_version_gte(v: &VersionId, major: usize, minor: usize, patch: usize) -> bool {
    (v.major, v.minor, v.patch) >= (major, minor, patch)
}
