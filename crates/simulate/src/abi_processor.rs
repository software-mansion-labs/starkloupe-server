use blockifier::abi::abi_utils::selector_from_name;
use data_decoder::utils::simplify_type_name;
use serde_json::{Map, Value};
use starknet_api::core::EntryPointSelector;
use std::borrow::Cow;
use walnut_shared::{EnumAbi, EventAbi, Parameter, StructAbi};

pub struct AbiProcessor {
    pub entry_point_selector: EntryPointSelector,
    pub entry_point_function_name: Option<String>,
    pub entry_point_interface_name: Option<String>,
    pub is_erc20_token: bool,
    pub function_arguments_names: Option<Vec<Cow<'static, str>>>,
    pub function_arguments_types: Option<Vec<Cow<'static, str>>>,
    pub function_return_result_types: Option<Vec<Cow<'static, str>>>,
    view_and_external_fn_names: Vec<String>,
    pub enum_abis: Vec<EnumAbi>,
    pub struct_abis: Vec<StructAbi>,
    pub event_abis: Vec<EventAbi>,
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
            view_and_external_fn_names: Vec::new(),
            enum_abis: Vec::new(),
            struct_abis: Vec::new(),
            event_abis: Vec::new(),
        }
    }

    pub fn process_abi(&mut self, abi: String) {
        let parsed_abi: Vec<Value> = serde_json::from_str(&abi).unwrap();
        self.process_abi_event(&parsed_abi);
        self.process_abi_struct(&parsed_abi);
        self.process_abi_enum(&parsed_abi);
        self.process_abi_internal(&parsed_abi);
        self.check_if_erc20_token();
    }

    fn process_abi_event(&mut self, abi_value_array: &Vec<Value>) {
        for item in abi_value_array {
            if let Value::Object(obj) = item {
                if obj.get("type") == Some(&Value::String("event".to_string())) {
                    if obj.get("kind") == Some(&Value::String("struct".to_string())) {
                        self.process_abi_event_members(obj);
                    } else if obj.get("kind") == Some(&Value::String("enum".to_string())) {
                        self.process_abi_event_variants(obj);
                    } else {
                        self.process_abi_event_data(obj);
                    }
                }
            }
        }
    }

    fn process_abi_event_members(&mut self, obj: &Map<String, Value>) {
        if let Some(Value::String(name)) = obj.get("name") {
            if let Some(Value::Array(event_members)) = obj.get("members") {
                let mut datas = Vec::new();
                for member in event_members {
                    if let Value::Object(member_obj) = member {
                        let member_name = member_obj.get("name").unwrap().as_str().unwrap();
                        let member_type = member_obj.get("type").unwrap().as_str().unwrap();
                        let simplified_member_type = simplify_type_name(member_type);
                        let data = Parameter {
                            name: member_name.to_string(),
                            type_name: simplified_member_type,
                        };
                        datas.push(data);
                    }
                }
                let event_item = EventAbi {
                    name: name.rsplit("::").next().unwrap().to_string(),
                    parameters: datas,
                };
                self.event_abis.push(event_item);
            }
        }
    }

    fn process_abi_event_variants(&mut self, obj: &Map<String, Value>) {
        if let Some(Value::String(_name)) = obj.get("name") {
            if let Some(Value::Array(event_members)) = obj.get("variants") {
                let mut datas = Vec::new();
                for member in event_members {
                    if let Value::Object(member_obj) = member {
                        let member_name = member_obj.get("name").unwrap().as_str().unwrap();
                        let member_type = member_obj.get("type").unwrap().as_str().unwrap();
                        let simplified_member_type = simplify_type_name(member_type);
                        let data = Parameter {
                            name: member_name.to_string(),
                            type_name: simplified_member_type,
                        };
                        datas.push(data);
                    }
                }
            }
        }
    }

    fn process_abi_event_data(&mut self, obj: &Map<String, Value>) {
        if let Some(Value::String(name)) = obj.get("name") {
            if let Some(Value::Array(event_datas)) = obj.get("data") {
                let mut datas = Vec::new();
                for member in event_datas {
                    if let Value::Object(member_obj) = member {
                        let member_name = member_obj.get("name").unwrap().as_str().unwrap();
                        let member_type = member_obj.get("type").unwrap().as_str().unwrap();
                        let simplified_member_type = simplify_type_name(member_type);
                        let data = Parameter {
                            name: member_name.to_string(),
                            type_name: simplified_member_type,
                        };
                        datas.push(data);
                    }
                }
                let event_item = EventAbi {
                    name: name.rsplit("::").next().unwrap().to_string(),
                    parameters: datas,
                };
                self.event_abis.push(event_item);
            }
        }
    }

    fn process_abi_struct(&mut self, abi_value_array: &Vec<Value>) {
        for item in abi_value_array {
            if let Value::Object(obj) = item {
                if obj.get("type") == Some(&Value::String("struct".to_string())) {
                    self.process_abi_struct_members(obj);
                }
            }
        }
    }

    fn process_abi_struct_members(&mut self, obj: &Map<String, Value>) {
        if let Some(Value::String(name)) = obj.get("name") {
            if let Some(Value::Array(struct_members)) = obj.get("members") {
                let mut datas = Vec::new();
                for member in struct_members {
                    if let Value::Object(member_obj) = member {
                        let member_name = member_obj.get("name").unwrap().as_str().unwrap();
                        let member_type = member_obj.get("type").unwrap().as_str().unwrap();
                        let simplified_member_type = simplify_type_name(member_type);
                        let data = Parameter {
                            name: member_name.to_string(),
                            type_name: simplified_member_type.to_string(),
                        };
                        datas.push(data);
                    }
                }
                let struct_item = StructAbi {
                    name: name.clone(),
                    parameters: datas,
                };
                self.struct_abis.push(struct_item);
            }
        }
    }

    fn process_abi_enum(&mut self, abi_value_array: &Vec<Value>) {
        for item in abi_value_array {
            if let Value::Object(obj) = item {
                if obj.get("type") == Some(&Value::String("enum".to_string())) {
                    self.process_abi_enum_variants(obj);
                }
            }
        }
    }

    fn process_abi_enum_variants(&mut self, obj: &Map<String, Value>) {
        if let Some(Value::String(name)) = obj.get("name") {
            if let Some(Value::Array(enum_variants)) = obj.get("variants") {
                let mut datas = Vec::new();
                for variant in enum_variants {
                    if let Value::Object(variant_obj) = variant {
                        let variant_name = variant_obj.get("name").unwrap().as_str().unwrap();
                        let variant_type = variant_obj.get("type").unwrap().as_str().unwrap();
                        let simplified_variant_type = simplify_type_name(variant_type);
                        let data = Parameter {
                            name: variant_name.to_string(),
                            type_name: simplified_variant_type.to_string(),
                        };
                        datas.push(data);
                    }
                }
                let enum_abi = EnumAbi {
                    name: simplify_type_name(name.as_str()),
                    parameters: datas,
                };
                self.enum_abis.push(enum_abi);
            }
        }
    }
    fn process_abi_internal(&mut self, abi_value_array: &Vec<Value>) {
        for item in abi_value_array {
            if let Value::Object(obj) = item {
                if obj.get("type") == Some(&Value::String("function".to_string())) {
                    self.process_abi_function(obj);
                } else if obj.get("type") == Some(&Value::String("interface".to_string())) {
                    self.process_abi_interface(obj);
                }
            }
        }
    }

    fn process_abi_function(&mut self, obj: &Map<String, Value>) {
        if obj.get("state_mutability") == Some(&Value::String("external".to_string())) {
            if let Some(Value::String(function_name)) = obj.get("name") {
                if self.entry_point_function_name.is_none() {
                    let selector = selector_from_name(function_name.as_str());
                    if self.entry_point_selector == selector {
                        self.entry_point_function_name = Some(function_name.clone());
                        self.process_function_arguments(obj);
                        self.process_function_results(obj);
                    }
                }
                self.view_and_external_fn_names.push(function_name.clone());
            }
        } else if obj.get("state_mutability") == Some(&Value::String("view".to_string())) {
            if let Some(Value::String(function_name)) = obj.get("name") {
                self.view_and_external_fn_names.push(function_name.clone());
                let selector = selector_from_name(function_name.as_str());
                if self.entry_point_selector == selector {
                    self.entry_point_function_name = Some(function_name.clone());
                    self.process_function_arguments(obj);
                    self.process_function_results(obj);
                }
            }
        }
    }

    fn process_function_arguments(&mut self, obj: &Map<String, Value>) {
        if let Some(Value::Array(inputs)) = obj.get("inputs") {
            for input in inputs {
                if let Value::Object(input_obj) = input {
                    if let Some(Value::String(name)) = input_obj.get("name") {
                        self.function_arguments_names
                            .get_or_insert(Vec::new())
                            .push(Cow::Owned(name.clone()));
                    }
                    if let Some(Value::String(arg_type)) = input_obj.get("type") {
                        let simplified_arg_type = simplify_type_name(arg_type.as_str());
                        self.function_arguments_types
                            .get_or_insert(Vec::new())
                            .push(Cow::Owned(simplified_arg_type));
                    }
                }
            }
        }
    }

    fn process_function_results(&mut self, obj: &Map<String, Value>) {
        if let Some(Value::Array(outputs)) = obj.get("outputs") {
            for output in outputs {
                if let Value::Object(output_obj) = output {
                    if let Some(Value::String(arg_type)) = output_obj.get("type") {
                        let simplified_arg_type = simplify_type_name(arg_type.as_str());
                        self.function_return_result_types
                            .get_or_insert(Vec::new())
                            .push(Cow::Owned(simplified_arg_type));
                    }
                }
            }
        }
    }

    fn process_abi_interface(&mut self, obj: &Map<String, Value>) {
        if let Some(Value::Array(items)) = obj.get("items") {
            if self.entry_point_function_name.is_none() {
                self.process_abi_internal(items);
                if self.entry_point_function_name.is_some() {
                    let current_entry_point_interface_name = match obj.get("name") {
                        Some(Value::String(name)) => Some(name),
                        _ => None,
                    };
                    self.entry_point_interface_name = current_entry_point_interface_name.cloned();
                }
            } else {
                self.process_abi_internal(items);
            }
        }
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
        for erc20_method in erc20_methods {
            if !erc20_method
                .iter()
                .any(|&s| self.view_and_external_fn_names.contains(&s.to_string()))
            {
                return;
            }
        }
        self.is_erc20_token = true;
    }
}
