use crate::abi::{
    get_enums, get_events, get_functions, get_structs, Enum, Event, EventData, EventKind, Input,
    Item, Output, StateMutability, Struct,
};
use crate::utils::simplify_type_name;
use blockifier::abi::abi_utils::selector_from_name;
use starknet_api::core::EntryPointSelector;
use std::{borrow::Cow, collections::HashSet};

pub struct AbiProcessor {
    pub entry_point_selector: EntryPointSelector,
    pub entry_point_function_name: Option<String>,
    pub entry_point_interface_name: Option<String>,
    pub is_erc20_token: bool,
    pub function_arguments_names: Option<Vec<Cow<'static, str>>>,
    pub function_arguments_types: Option<Vec<Cow<'static, str>>>,
    pub function_return_result_types: Option<Vec<Cow<'static, str>>>,
    view_and_external_fn_names: HashSet<String>,
    pub enum_abis: Vec<Enum>,
    pub struct_abis: Vec<Struct>,
    pub event_abis: Vec<Event>,
}

impl AbiProcessor {
    pub fn new(entry_point_selector: EntryPointSelector) -> Self {
        AbiProcessor {
            entry_point_selector,
            entry_point_function_name: None,
            entry_point_interface_name: None,
            is_erc20_token: false,
            function_arguments_names: None,
            function_arguments_types: None,
            function_return_result_types: None,
            view_and_external_fn_names: HashSet::new(),
            struct_abis: Vec::new(),
            enum_abis: Vec::new(),
            event_abis: Vec::new(),
        }
    }

    pub fn process_abi(&mut self, abi: &str) {
        let items: Vec<Item> = serde_json::from_str(abi).expect("Invalid ABI JSON format");
        self.process_abi_event(&items);
        self.process_abi_struct(&items);
        self.process_abi_enum(&items);
        self.process_abi_internal(&items);
        self.check_if_erc20_token();
    }

    fn process_abi_event(&mut self, items: &[Item]) {
        for mut event in get_events(items) {
            event.name = event
                .name
                .rsplit("::")
                .next()
                .unwrap_or(&event.name)
                .to_string();
            match &mut event.data {
                EventData::Cairo2 { kind } => match kind {
                    EventKind::Struct { members } => {
                        members.iter_mut().for_each(|member| {
                            member.ty = simplify_type_name(&member.ty);
                        });
                    }
                    EventKind::Enum { variants } => {
                        variants.iter_mut().for_each(|variant| {
                            variant.ty = simplify_type_name(&variant.ty);
                        });
                    }
                },
                EventData::Cairo1 { inputs } => {
                    inputs.iter_mut().for_each(|input| {
                        input.ty = simplify_type_name(&input.ty);
                    });
                }
            }
            self.event_abis.push(event);
        }
    }

    fn process_abi_struct(&mut self, items: &[Item]) {
        for mut s in get_structs(items) {
            s.name = simplify_type_name(&s.name);
            let _ = &mut s
                .members
                .iter_mut()
                .for_each(|member| member.ty = simplify_type_name(&member.ty));
            self.struct_abis.push(s);
        }
    }

    fn process_abi_enum(&mut self, items: &[Item]) {
        for mut e in get_enums(items) {
            e.name = simplify_type_name(&e.name);
            let _ = &mut e
                .variants
                .iter_mut()
                .for_each(|variant| variant.ty = simplify_type_name(&variant.ty));
            self.enum_abis.push(e);
        }
    }

    fn process_abi_internal(&mut self, items: &[Item]) {
        for func in get_functions(items) {
            if func.state_mutability == StateMutability::External
                || func.state_mutability == StateMutability::View
            {
                let selector = selector_from_name(&func.name);
                if self.entry_point_function_name.is_none() && self.entry_point_selector == selector
                {
                    self.entry_point_function_name = Some(func.name.clone());
                    self.process_function_arguments(&func.inputs);
                    self.process_function_results(&func.outputs);
                }
                self.view_and_external_fn_names.insert(func.name.clone());
            }
        }

        for interface in items.iter().filter_map(|item| {
            if let Item::Interface(i) = item {
                Some(i)
            } else {
                None
            }
        }) {
            if self.entry_point_function_name.is_some() && self.entry_point_interface_name.is_none()
            {
                self.entry_point_interface_name = Some(interface.name.clone());
            }
        }
    }

    fn process_function_arguments(&mut self, inputs: &[Input]) {
        let (names, types): (Vec<_>, Vec<_>) = inputs
            .iter()
            .map(|input| {
                (
                    Cow::Owned(input.name.clone()),
                    Cow::Owned(simplify_type_name(&input.ty)),
                )
            })
            .unzip();

        self.function_arguments_names
            .get_or_insert_with(Vec::new)
            .extend(names);

        self.function_arguments_types
            .get_or_insert_with(Vec::new)
            .extend(types);
    }

    fn process_function_results(&mut self, outputs: &[Output]) {
        let types = outputs
            .iter()
            .map(|output| Cow::Owned(simplify_type_name(&output.ty)));

        self.function_return_result_types
            .get_or_insert_with(Vec::new)
            .extend(types);
    }

    fn check_if_erc20_token(&mut self) {
        let erc20_methods = vec![
            vec!["name"],
            vec!["symbol"],
            vec!["decimals"],
            vec!["allowance"],
            vec!["transfer"],
            vec!["approve"],
            vec!["increase_allowance", "increaseAllowance"],
            vec!["decrease_allowance", "decreaseAllowance"],
            vec!["transfer_from", "transferFrom"],
            vec!["balance_of", "balanceOf"],
            vec!["total_supply", "totalSupply"],
        ];
        for methods in erc20_methods {
            if !methods
                .iter()
                .any(|&s| self.view_and_external_fn_names.contains(s))
            {
                return;
            }
        }
        self.is_erc20_token = true;
    }
}
