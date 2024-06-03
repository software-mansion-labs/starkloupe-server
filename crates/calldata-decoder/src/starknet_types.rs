use std::fmt;

#[derive(Debug)]
pub enum EDataType {
    Primitive(EPrimitiveType),
    Array(Box<EDataType>),
    Struct(String),
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

impl fmt::Display for EDataType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            EDataType::Primitive(primitive) => write!(f, "{:?}", primitive),
            EDataType::Array(inner_type) => write!(f, "Array<{:?}>", inner_type),
            EDataType::Struct(name) => write!(f, "{}", name),
        }
    }
}

impl EPrimitiveType {
    pub fn from_str(s: &str) -> Option<Self> {
        match s {
            "core::integer::u8" => Some(Self::U8),
            "core::integer::u16" => Some(Self::U16),
            "core::integer::u32" => Some(Self::U32),
            "core::integer::u64" => Some(Self::U64),
            "core::integer::u128" => Some(Self::U128),
            "core::integer::usize" => Some(Self::Usize),
            "core::integer::i8" => Some(Self::I8),
            "core::integer::i16" => Some(Self::I16),
            "core::integer::i32" => Some(Self::I32),
            "core::integer::i64" => Some(Self::I64),
            "core::integer::i128" => Some(Self::I128),
            "core::bool" => Some(Self::Bool),
            "felt" => Some(Self::Felt),
            "core::felt252" => Some(Self::Felt252),
            "core::starknet::contract_address::ContractAddress" => Some(Self::ContractAddress),
            "core::staknet::eth_address::EthAddress" => Some(Self::EthAddress),
            "core::starknet::class_hash::ClassHash" => Some(Self::ClassHash),
            "core::bytes_31::bytes31" => Some(Self::Bytes31),
            _ => None,
        }
    }
}

impl EDataType {
    pub fn from_str(s: &str) -> Self {
        if let Some(primitive) = EPrimitiveType::from_str(s) {
            Self::Primitive(primitive)
        } else if s.starts_with("core::array::Array::<")
            || s.starts_with("@core::array::Array::<")
            || s.starts_with("core::array::Span::<")
        {
            let inner_type = &s[s.find("::<").unwrap() + 3..s.len() - 1];
            Self::Array(Box::new(Self::from_str(inner_type)))
        } else {
            Self::Struct(s.to_string())
        }
    }
}
