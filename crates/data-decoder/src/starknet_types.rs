use regex::Regex;
use std::fmt;
use walnut_shared::EnumItems;

#[derive(Debug)]
pub enum EDataType {
    System(ESystemType),
    Primitive(EPrimitiveType),
    Array(Box<EDataType>),
    Struct(String),
    Tuple(Vec<String>),
    SystemEnum(EEnumType),
    UserEnum(String),
}

#[derive(Debug)]
pub enum ESystemType {
    Const,
    Step,
    Hole,
    RangeCheck,
    RangeCheck96,
    Pedersen,
    Bitwise,
    EcOp,
    System,
    GasBuiltin,
    Poseidon,
    Unit,
    Snapshot,
    ComponentState,
}

#[derive(Debug)]
pub enum EPrimitiveType {
    U8,
    U16,
    U32,
    U64,
    U128,
    Usize,
    I8,
    I16,
    I32,
    I64,
    I128,
    Bool,
    Felt,
    Felt252,
    ContractAddress,
    EthAddress,
    ClassHash,
    Bytes31,
}

#[derive(Debug)]
pub enum EEnumType {
    PanicResult,
    Option,
    Result,
}

impl fmt::Display for EDataType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            EDataType::System(system) => write!(f, "{:?}", system),
            EDataType::Primitive(primitive) => write!(f, "{:?}", primitive),
            EDataType::Array(inner_type) => write!(f, "Array<{:?}>", inner_type),
            EDataType::Struct(name) => write!(f, "{}", name),
            EDataType::Tuple(inner_types) => {
                let formatted_types: Vec<String> =
                    inner_types.iter().map(|t| format!("{:?}", t)).collect();
                write!(f, "Tuple<{}>", formatted_types.join(", "))
            }
            EDataType::SystemEnum(name) => write!(f, "{:?}", name),
            EDataType::UserEnum(name) => write!(f, "{}", name),
        }
    }
}

impl ESystemType {
    pub fn from_str(s: &str) -> Option<Self> {
        match s {
            "Const" => Some(Self::Const),
            "Step" => Some(Self::Step),
            "Hole" => Some(Self::Hole),
            "RangeCheck" => Some(Self::RangeCheck),
            "RangeCheck96" => Some(Self::RangeCheck96),
            "Pedersen" => Some(Self::Pedersen),
            "Bitwise" => Some(Self::Bitwise),
            "EcOp" => Some(Self::EcOp),
            "System" => Some(Self::System),
            "GasBuiltin" => Some(Self::GasBuiltin),
            "Poseidon" => Some(Self::Poseidon),
            "Unit" => Some(Self::Unit),
            _ if s.contains("()") => Some(Self::Unit),
            _ if s.contains("Snapshot") => Some(Self::Snapshot),
            _ if s.contains("ComponentState") => Some(Self::ComponentState),
            _ => None,
        }
    }
}

impl EPrimitiveType {
    pub fn from_str(s: &str) -> Option<Self> {
        let last_segment = s.rsplit("::").next().unwrap_or(s);

        match last_segment {
            "u8" => Some(Self::U8),
            "u16" => Some(Self::U16),
            "u32" => Some(Self::U32),
            "u64" => Some(Self::U64),
            "u128" => Some(Self::U128),
            "usize" => Some(Self::Usize),
            "i8" => Some(Self::I8),
            "i16" => Some(Self::I16),
            "i32" => Some(Self::I32),
            "i64" => Some(Self::I64),
            "i128" => Some(Self::I128),
            "bool" => Some(Self::Bool),
            "felt" => Some(Self::Felt),
            "felt252" => Some(Self::Felt252),
            "ContractAddress" => Some(Self::ContractAddress),
            "EthAddress" => Some(Self::EthAddress),
            "ClassHash" => Some(Self::ClassHash),
            "bytes31" => Some(Self::Bytes31),
            _ => None,
        }
    }
}

impl EEnumType {
    pub fn from_str(s: &str) -> Option<Self> {
        // Handle predefined enums like PanicResult, Option, Result
        match s {
            s if s.contains("PanicResult") => Some(Self::PanicResult),
            s if s.contains("Option") => Some(Self::Option),
            s if s.contains("Result") => Some(Self::Result),
            _ => None,
        }
    }
}
impl EDataType {
    pub fn from_str(s: &str, enum_items: Option<&Vec<EnumItems>>) -> Self {
        if let Some(primitive) = EPrimitiveType::from_str(s) {
            return Self::Primitive(primitive);
        }
        if s.starts_with("core::array::Array::<")
            || s.starts_with("@core::array::Array::<")
            || s.starts_with("core::array::Span::<")
        {
            let inner_type = &s[s.find("::<").unwrap() + 3..s.len() - 1];
            return Self::Array(Box::new(Self::from_str(inner_type, enum_items)));
        }
        if s.starts_with("Tuple<") {
            let inner_types = extract_inner_types(s);
            return Self::Tuple(inner_types);
        }
        if let Some(enum_type) = EEnumType::from_str(s) {
            return Self::SystemEnum(enum_type);
        }
        if let Some(items) = enum_items {
            if items.iter().any(|item| item.name == s) {
                return Self::UserEnum(s.to_string());
            }
        }
        if let Some(system) = ESystemType::from_str(s) {
            return Self::System(system);
        }
        Self::Struct(s.to_string())
    }
}

fn extract_inner_types(data_type: &str) -> Vec<String> {
    let re_inner_type = Regex::new(r"<\s*\(?\s*(.*[^\s\)])\s*\)?\s*>").unwrap();
    if let Some(captures) = re_inner_type.captures(data_type) {
        let inner_content = captures.get(1).map_or("", |m| m.as_str());
        return inner_content
            .split(',')
            .map(|s| s.trim().to_string())
            .collect();
    }
    vec![]
}
