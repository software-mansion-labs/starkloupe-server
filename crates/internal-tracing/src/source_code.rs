use anyhow::Result;
use starknet_api::core::ClassHash;
use std::{
    collections::{HashMap, HashSet},
    fs,
};

pub type SourceCode = HashMap<String, HashMap<String, String>>;

pub fn fetch_source_code(
    used_source_files: HashMap<ClassHash, HashSet<String>>,
) -> Result<SourceCode> {
    let mut result_map: SourceCode = HashMap::new();

    for (class_hash, file_paths) in used_source_files {
        let class_hash_str = class_hash.to_string();
        let mut file_map: HashMap<String, String> = HashMap::new();
        for file_path in file_paths {
            let full_path = format!("precompiled/{}/source_code/{}", class_hash_str, file_path);
            let content = fs::read_to_string(full_path)?;
            file_map.insert(file_path, content);
        }
        result_map.insert(class_hash_str, file_map);
    }

    Ok(result_map)
}
