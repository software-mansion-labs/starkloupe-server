//Defintion for abi copied from
//https://github.com/starkware-libs/cairo/blob/v2.10.1/crates/cairo-lang-starknet-classes/src/abi.rs

use serde::{Deserialize, Serialize};
use std::borrow::Cow;

/// Enum of contract item ABIs.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, Hash)]
#[serde(tag = "type")]
pub enum Item {
    #[serde(rename = "function")]
    Function(Function),
    #[serde(rename = "constructor")]
    Constructor(Constructor),
    #[serde(rename = "l1_handler")]
    L1Handler(L1Handler),
    #[serde(rename = "event")]
    Event(Event),
    #[serde(rename = "struct")]
    Struct(Struct),
    #[serde(rename = "enum")]
    Enum(Enum),
    #[serde(rename = "interface")]
    Interface(Interface),
    #[serde(rename = "impl")]
    Impl(Imp),
}

/// Contract interface ABI.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, Hash)]
pub struct Interface {
    pub name: String,
    pub items: Vec<Item>,
}

/// Contract impl ABI.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, Hash)]
pub struct Imp {
    pub name: String,
    pub interface_name: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, Hash)]
pub enum StateMutability {
    #[serde(rename = "external")]
    External,
    #[serde(rename = "view")]
    View,
}

/// Contract function ABI.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, Hash)]
pub struct Function {
    pub name: String,
    pub inputs: Vec<Input>,
    pub outputs: Vec<Output>,
    pub state_mutability: StateMutability,
}

/// Contract constructor ABI.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, Hash)]
pub struct Constructor {
    pub name: String,
    pub inputs: Vec<Input>,
}

/// Contract L1 handler ABI.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, Hash)]
pub struct L1Handler {
    pub name: String,
    pub inputs: Vec<Input>,
    pub outputs: Vec<Output>,
    pub state_mutability: StateMutability,
}

/// Contract event.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, Hash)]
pub struct Event {
    pub name: String,
    #[serde(flatten)]
    pub data: EventData,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, Hash)]
#[serde(untagged)]
pub enum EventData {
    Cairo2 {
        #[serde(flatten)]
        kind: EventKind,
    },
    Cairo1 {
        inputs: Vec<EventFieldLegacy>,
    },
}

/// Contract event kind.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, Hash)]
#[serde(tag = "kind")]
pub enum EventKind {
    #[serde(rename = "struct")]
    Struct { members: Vec<EventField> },
    #[serde(rename = "enum")]
    Enum { variants: Vec<EventField> },
}

/// Contract event field (member/variant).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, Hash)]
pub struct EventField {
    pub name: String,
    #[serde(rename = "type")]
    pub ty: String,
    pub kind: EventFieldKind,
}

/// Describes how to serialize the event's field.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Serialize, Deserialize, Hash)]
pub enum EventFieldKind {
    #[serde(rename = "key")]
    KeySerde,
    #[serde(rename = "data")]
    DataSerde,
    #[serde(rename = "nested")]
    Nested,
    #[serde(rename = "flat")]
    Flat,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, Hash)]
pub struct EventFieldLegacy {
    pub name: String,
    #[serde(rename = "type")]
    pub ty: String,
}

/// Function input ABI.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, Hash)]
pub struct Input {
    pub name: String,
    #[serde(rename = "type")]
    pub ty: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub members: Option<Cow<'static, [StructMember]>>, // For struct types
    #[serde(skip_serializing_if = "Option::is_none")]
    pub variants: Option<Cow<'static, [EnumVariant]>>, // For enum types
}

/// Function Output ABI.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, Hash)]
pub struct Output {
    #[serde(rename = "type")]
    pub ty: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub members: Option<Cow<'static, [StructMember]>>, // For struct types
    #[serde(skip_serializing_if = "Option::is_none")]
    pub variants: Option<Cow<'static, [EnumVariant]>>, // For enum types
}

/// Struct ABI.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, Hash)]
pub struct Struct {
    pub name: String,
    pub members: Vec<StructMember>,
}

/// Struct member.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, Hash)]
pub struct StructMember {
    pub name: String,
    #[serde(rename = "type")]
    pub ty: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub members: Option<Cow<'static, [StructMember]>>, // For nested struct types
    #[serde(skip_serializing_if = "Option::is_none")]
    pub variants: Option<Cow<'static, [EnumVariant]>>, // For enum types
}

/// Enum ABI.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, Hash)]
pub struct Enum {
    pub name: String,
    pub variants: Vec<EnumVariant>,
}

/// Enum variant.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, Hash)]
pub struct EnumVariant {
    pub name: String,
    #[serde(rename = "type")]
    pub ty: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub members: Option<Cow<'static, [StructMember]>>, // For struct types in enum variants
    #[serde(skip_serializing_if = "Option::is_none")]
    pub variants: Option<Cow<'static, [EnumVariant]>>, // For nested enum types in enum variants
}

pub fn get_functions(items: &[Item]) -> Vec<Function> {
    let mut functions = Vec::new();
    for item in items {
        match item {
            Item::Function(f) => functions.push(f.clone()),
            Item::Interface(interface) => {
                functions.extend(get_functions(&interface.items));
            }
            _ => {}
        }
    }
    functions
}

pub fn get_structs(items: &[Item]) -> Vec<Struct> {
    let mut structs = Vec::new();
    for item in items {
        match item {
            Item::Struct(s) => structs.push(s.clone()),
            Item::Interface(interface) => {
                structs.extend(get_structs(&interface.items));
            }
            _ => {}
        }
    }
    structs
}

pub fn get_enums(items: &[Item]) -> Vec<Enum> {
    let mut enums = Vec::new();
    for item in items {
        match item {
            Item::Enum(e) => enums.push(e.clone()),
            Item::Interface(interface) => {
                enums.extend(get_enums(&interface.items));
            }
            _ => {}
        }
    }
    enums
}

pub fn get_events(items: &[Item]) -> Vec<Event> {
    let mut events = Vec::new();
    for item in items {
        match item {
            Item::Event(e) => events.push(e.clone()),

            Item::Interface(interface) => {
                events.extend(get_events(&interface.items));
            }
            _ => {}
        }
    }
    events
}
