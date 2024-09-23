use blockifier::abi::abi_utils::selector_from_name;
use serde_json::{Map, Value};
use starknet_api::core::EntryPointSelector;
use walnut_shared::{Datas, EnumItems, EventItems, StructItems};

pub struct AbiProcessor {
    pub entry_point_selector: EntryPointSelector,
    pub entry_point_function_name: Option<String>,
    pub entry_point_interface_name: Option<String>,
    pub is_erc20_token: bool,
    pub function_arguments_names: Option<Vec<String>>,
    pub function_arguments_types: Option<Vec<String>>,
    pub function_return_result_types: Option<Vec<String>>,
    view_and_external_fn_names: Vec<String>,
    pub struct_items: Vec<StructItems>,
    pub enum_items: Vec<EnumItems>,
    pub event_items: Vec<EventItems>,
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
            struct_items: Vec::new(),
            enum_items: Vec::new(),
            event_items: Vec::new(),
        }
    }

    pub fn process_abi(&mut self, abi: String) {
        self.process_abi_event(&serde_json::from_str(abi.as_str()).unwrap());
        self.process_abi_struct(&serde_json::from_str(abi.as_str()).unwrap());
        self.process_abi_enum(&serde_json::from_str(abi.as_str()).unwrap());
        self.process_abi_internal(&serde_json::from_str(abi.as_str()).unwrap());
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
                        let data = Datas {
                            names: member_name.to_string(),
                            types: member_type.to_string(),
                        };
                        datas.push(data);
                    }
                }
                let event_item = EventItems {
                    name: name.rsplit("::").next().unwrap().to_string(),
                    members: datas,
                };
                self.event_items.push(event_item);
            }
        }
    }

    fn process_abi_event_variants(&mut self, obj: &Map<String, Value>) {
        if let Some(Value::String(name)) = obj.get("name") {
            if let Some(Value::Array(event_members)) = obj.get("variants") {
                let mut datas = Vec::new();
                for member in event_members {
                    if let Value::Object(member_obj) = member {
                        let member_name = member_obj.get("name").unwrap().as_str().unwrap();
                        let member_type = member_obj.get("type").unwrap().as_str().unwrap();
                        let data = Datas {
                            names: member_name.to_string(),
                            types: member_type.to_string(),
                        };
                        datas.push(data);
                    }
                }
                let event_item = EventItems {
                    name: name.clone(),
                    members: datas,
                };
                //self.event_items.push(event_item);
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
                        let data = Datas {
                            names: member_name.to_string(),
                            types: member_type.to_string(),
                        };
                        datas.push(data);
                    }
                }
                let event_item = EventItems {
                    name: name.rsplit("::").next().unwrap().to_string(),
                    members: datas,
                };
                self.event_items.push(event_item);
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
                        let data = Datas {
                            names: member_name.to_string(),
                            types: member_type.to_string(),
                        };
                        datas.push(data);
                    }
                }
                let struct_item = StructItems {
                    name: name.clone(),
                    members: datas,
                };
                self.struct_items.push(struct_item);
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
                        let data = Datas {
                            names: variant_name.to_string(),
                            types: variant_type.to_string(),
                        };
                        datas.push(data);
                    }
                }
                let enum_items = EnumItems {
                    name: name.clone(),
                    members: datas,
                };
                self.enum_items.push(enum_items);
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
                            .push(name.clone());
                    }
                    if let Some(Value::String(arg_type)) = input_obj.get("type") {
                        self.function_arguments_types
                            .get_or_insert(Vec::new())
                            .push(arg_type.clone());
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
                        self.function_return_result_types
                            .get_or_insert(Vec::new())
                            .push(arg_type.clone());
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
