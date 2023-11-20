use std::collections::BTreeMap;
use std::collections::HashMap;

use serde::Deserialize;
use serde::Serialize;
use serde_json::Error;
use serde_json::Value;

const STRUCT_ENTRY: &str = "struct";
const FUNCTION_ENTRY: &str = "function";
const CONSTRUCTOR_ENTRY: &str = "constructor";
const L1_HANDLER_ENTRY: &str = "l1_handler";
const EVENT_ENTRY: &str = "event";

pub struct AbiParser {
    grouped: HashMap<String, Vec<Value>>,
}

impl AbiParser {
    pub fn new(abi_json_str: &str) -> Result<Self, Error> {
        let abi_list: Vec<Value> = serde_json::from_str(abi_json_str)?;

        let mut grouped: HashMap<String, Vec<Value>> = HashMap::new();

        // Group each entry in the ABI list by type.
        for entry in abi_list {
            if let Some(entry_type) = entry.get("type").and_then(Value::as_str) {
                grouped
                    .entry(entry_type.to_string())
                    .or_insert_with(Vec::new)
                    .push(entry);
            }
        }

        Ok(AbiParser { grouped })
    }

    pub fn parse(&self) -> Result<Abi, Error> {
        // TODO: parse JSON to Abi
        let defined_structures = HashMap::new();
        let structs = self.grouped.get(STRUCT_ENTRY);
        Ok(Abi {
            defined_structures,
            functions: HashMap::new(),
            events: HashMap::new(),
            constructor: None,
            l1_handler: None,
        })
    }
}

pub struct Abi {
    pub defined_structures: HashMap<String, StructType>,
    pub functions: HashMap<String, Function>,
    pub constructor: Option<Function>,
    pub l1_handler: Option<Function>,
    pub events: HashMap<String, Event>,
}

pub struct Event {
    pub name: String,
    pub data: BTreeMap<String, CairoType>,
}

pub struct Function {
    pub name: String,
    pub inputs: BTreeMap<String, CairoType>,
    pub outputs: BTreeMap<String, CairoType>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
#[cfg_attr(feature = "no_unknown_fields", serde(deny_unknown_fields))]
pub enum CairoType {
    FeltType,
    BoolType,
    TupleType(TupleType),
    NamedTupleType(NamedTupleType),
    ArrayType(Box<ArrayType>),
    StructType(Box<StructType>),
    EnumType(Box<EnumType>),
    OptionType(Box<OptionType>),
    UintType(UintType),
    TypeIdentifier,
    EventType,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "no_unknown_fields", serde(deny_unknown_fields))]
pub struct TupleType {
    pub types: Vec<CairoType>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "no_unknown_fields", serde(deny_unknown_fields))]
pub struct NamedTupleType {
    pub types: HashMap<String, CairoType>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "no_unknown_fields", serde(deny_unknown_fields))]
pub struct ArrayType {
    pub inner_type: CairoType,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "no_unknown_fields", serde(deny_unknown_fields))]
pub struct StructType {
    pub name: String,
    pub types: BTreeMap<String, CairoType>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "no_unknown_fields", serde(deny_unknown_fields))]
pub struct EnumType {
    pub name: String,
    pub variants: BTreeMap<String, CairoType>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "no_unknown_fields", serde(deny_unknown_fields))]
pub struct OptionType {
    pub inner_type: CairoType,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "no_unknown_fields", serde(deny_unknown_fields))]
pub struct UintType {
    pub bits: u8,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "no_unknown_fields", serde(deny_unknown_fields))]
pub struct TypeIdentifier {
    pub name: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "no_unknown_fields", serde(deny_unknown_fields))]
pub struct EventType {
    pub name: String,
    pub types: BTreeMap<String, CairoType>,
}
