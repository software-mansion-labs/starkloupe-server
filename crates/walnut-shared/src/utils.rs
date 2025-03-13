#[inline(always)]
pub fn simplify_type_name(type_str: &str) -> String {
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
    format!("{}<{}>", main_type_name, parsed_inner_type)
}

#[inline(always)]
fn extract_last_segment(t: &str) -> String {
    t.rsplit("::")
        .find(|&part| !part.is_empty())
        .unwrap_or(t)
        .to_string()
}
