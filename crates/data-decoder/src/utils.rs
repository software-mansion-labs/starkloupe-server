use crate::constants::SKIP_BUILTIN_TYPES;

#[inline(always)]
pub fn skip_builtin_type_declaration(type_name: &str) -> bool {
    SKIP_BUILTIN_TYPES
        .iter()
        .any(|&builtin| type_name.starts_with(builtin))
}

