use blockifier::abi::abi_utils::selector_from_name;
use serde_json::{Map, Value};
use starknet_api::core::EntryPointSelector;

pub struct AbiProcessor {
    pub entry_point_selector: EntryPointSelector,
    pub entry_point_function_name: Option<String>,
    pub entry_point_interface_name: Option<String>,
    pub is_erc20_token: bool,
    view_and_external_fn_names: Vec<String>,
}

impl AbiProcessor {
    pub fn new(entry_point_selector: EntryPointSelector) -> Self {
        AbiProcessor {
            entry_point_selector,
            entry_point_function_name: None,
            entry_point_interface_name: None,
            is_erc20_token: false,
            view_and_external_fn_names: Vec::new(),
        }
    }

    pub fn process_abi(&mut self, abi: String) {
        self.process_abi_internal(&serde_json::from_str(abi.as_str()).unwrap());
        self.check_if_erc20_token();
    }

    fn process_abi_internal(&mut self, abi_value: &Value) {
        if let Value::Array(array) = abi_value {
            for item in array {
                if let Value::Object(obj) = item {
                    if obj.get("type") == Some(&Value::String("function".to_string())) {
                        self.process_abi_function(obj);
                    } else if obj.get("type") == Some(&Value::String("interface".to_string())) {
                        self.process_abi_interface(obj);
                    }
                }
            }
        }
    }

    fn process_abi_function(&mut self, obj: &Map<String, Value>) {
        if obj.get("state_mutability") == Some(&Value::String("external".to_string())) {
            if let Some(Value::String(function_name)) = obj.get("name") {
                if self.entry_point_function_name.is_none() {
                    let selector = selector_from_name(function_name.as_str());
                    if selector == self.entry_point_selector {
                        self.entry_point_function_name = Some(function_name.clone());
                    }
                }
                self.view_and_external_fn_names.push(function_name.clone());
            }
        } else if obj.get("state_mutability") == Some(&Value::String("view".to_string())) {
            if let Some(Value::String(function_name)) = obj.get("name") {
                self.view_and_external_fn_names.push(function_name.clone());
            }
        }
    }

    fn process_abi_interface(&mut self, obj: &Map<String, Value>) {
        if let Some(Value::Array(items)) = obj.get("items") {
            if self.entry_point_function_name.is_none() {
                self.process_abi_internal(&Value::Array(items.clone()));
                if self.entry_point_function_name.is_some() {
                    let current_entry_point_interface_name = match obj.get("name") {
                        Some(Value::String(name)) => Some(name),
                        _ => None,
                    };
                    self.entry_point_interface_name = current_entry_point_interface_name.cloned();
                }
            } else {
                self.process_abi_internal(&Value::Array(items.clone()));
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
