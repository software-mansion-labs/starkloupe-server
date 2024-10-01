use serde_json::{json, map::Map, Value};

pub fn create_result_obj(
    names: &[String],
    index: usize,
    data_type: &str,
    value: Value, // Change this to accept Value directly
) -> Map<String, Value> {
    let mut result_obj = Map::new();

    if !names.is_empty() && !names[index].is_empty() {
        result_obj.insert("name".to_string(), json!(names[index]));
    }
    result_obj.insert("type".to_string(), json!(data_type));

    // Directly insert the value (which could be Object, String, Array, etc.)
    result_obj.insert("value".to_string(), value);

    result_obj
}
