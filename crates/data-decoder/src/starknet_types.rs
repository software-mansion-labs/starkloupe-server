use std::fmt;
use walnut_shared::abi::Enum;

#[derive(Debug)]
pub enum EDataType {
    Primitive(EPrimitiveType),
    Array(Box<EDataType>),
    Struct(String),
    Tuple(Vec<String>),
    UserEnum(String),
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
    Panic,
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
            EDataType::Primitive(primitive) => write!(f, "{:?}", primitive),
            EDataType::Array(inner_type) => write!(f, "Array<{}>", inner_type),
            EDataType::Struct(name) => write!(f, "{}", name),
            EDataType::Tuple(inner_types) => {
                let formatted_types: Vec<String> =
                    inner_types.iter().map(|t| format!("{:?}", t)).collect();
                write!(f, "({})", formatted_types.join(", "))
            }
            EDataType::UserEnum(name) => write!(f, "{}", name),
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
            "Panic" => Some(Self::Panic),
            _ => None,
        }
    }
}

impl EEnumType {
    pub fn from_str(s: &str) -> Option<Self> {
        // Handle predefined enums like PanicResult, Option, Result
        match s {
            s if s.starts_with("PanicResult") => Some(Self::PanicResult),
            s if s.starts_with("Option") => Some(Self::Option),
            s if s.starts_with("Result") => Some(Self::Result),
            _ => None,
        }
    }
}
impl EDataType {
    pub fn from_str(s: &str, enum_abis: Option<&[Enum]>) -> Self {
        if let Some(primitive) = EPrimitiveType::from_str(s) {
            return Self::Primitive(primitive);
        }
        if let Some(inner) = s
            .strip_prefix("Array<")
            .or_else(|| s.strip_prefix("Span<"))
            .and_then(|s| s.strip_suffix('>'))
        {
            return Self::Array(Box::new(Self::from_str(inner, enum_abis)));
        }
        if let Some(stripped) = s.strip_prefix('[').and_then(|s| s.strip_suffix(']')) {
            if let Some((inner, _)) = stripped.split_once(';') {
                return Self::Array(Box::new(Self::from_str(inner.trim(), enum_abis)));
            }
        }
        if s.starts_with("Tuple") || (s.starts_with("(") && s.ends_with(")")) {
            let inner_types = extract_inner_types(s);
            return Self::Tuple(inner_types);
        }
        if let Some(items) = enum_abis {
            if items.iter().any(|item| item.name == s) {
                return Self::UserEnum(s.to_string());
            }
        }
        Self::Struct(s.to_string())
    }
}

fn extract_inner_types(data_type: &str) -> Vec<String> {
    let inner_content = data_type
        .strip_prefix("Tuple<")
        .or_else(|| data_type.strip_prefix('('))
        .and_then(|s| s.strip_suffix('>').or_else(|| s.strip_suffix(')')))
        .unwrap_or(data_type);

    inner_content
        .split(',')
        .map(|s| {
            s.trim_matches(|c: char| c.is_whitespace() || c == '"')
                .to_string()
        })
        .collect()
}
