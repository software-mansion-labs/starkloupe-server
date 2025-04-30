use crate::constants::SKIP_BUILTIN_TYPES;
use tracing::warn;

#[inline(always)]
pub fn skip_builtin_type_declaration(type_name: &str) -> bool {
    SKIP_BUILTIN_TYPES
        .iter()
        .any(|&builtin| type_name.starts_with(builtin))
}

const AVG_SIZE_PER_ELEMENT: usize = 512; // bytes
const MAX_ESTIMATED_BYTES: usize = 1024 * 1024 * 1024; // 1GB

/// Checks if the estimated memory allocation for an array of given length exceeds the allowed maximum
/// With 512 bytes per element, the maximum number of elements that fits into 1GB is: 1GB / 512 bytes = 2,097,152 elements
pub fn is_allocation_safe(array_length: usize) -> bool {
    let estimated_bytes = array_length.saturating_mul(AVG_SIZE_PER_ELEMENT);

    if estimated_bytes > MAX_ESTIMATED_BYTES {
        warn!(
            "Skipping allocation: estimated {} bytes for array_length = {}",
            estimated_bytes, array_length
        );
        return false;
    }

    true
}
