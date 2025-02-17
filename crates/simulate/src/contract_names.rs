use futures::future::join_all;
use serde_json::Value;
use starknet::core::types::Felt;
use starknet::macros::selector;
use starknet_api::core::{ChainId, ContractAddress};
use starknet_old::core::types as starknet_old_types;
use starknet_providers::jsonrpc::HttpTransport;
use starknet_providers::JsonRpcClient;
use starknet_providers::Provider;
use std::collections::{HashMap, HashSet};
use tracing::info;
use walnut_shared::felt_to_field_element;
use walnut_shared::{bytes_to_text, get_voyager_api_url};

use crate::contract_calls_map::ContractCallsMap;

pub struct ContractNamesFetcher {
    provider_client: JsonRpcClient<HttpTransport>,
    voyager_api_url: Option<String>,
    pub contract_addresses: HashSet<ContractAddress>,
    pub token_addresses: HashSet<ContractAddress>,
    pub token_contract_names: HashMap<ContractAddress, ContractName>,
    pub contract_names: HashMap<ContractAddress, ContractName>,
}

#[derive(Debug)]
pub struct ContractName {
    pub name: Option<String>,
    pub symbol: Option<String>,
}

impl ContractNamesFetcher {
    pub fn new(provider_client: JsonRpcClient<HttpTransport>, chain_id: &ChainId) -> Self {
        let voyager_api_url = get_voyager_api_url(chain_id).map(|url| url.to_string());
        ContractNamesFetcher {
            provider_client,
            voyager_api_url,
            contract_addresses: HashSet::new(),
            token_addresses: HashSet::new(),
            token_contract_names: HashMap::new(),
            contract_names: HashMap::new(),
        }
    }

    pub async fn fetch_single_contract_name(
        &self,
        contract_address: String,
    ) -> Option<ContractName> {
        match &self.voyager_api_url {
            Some(voyager_api_url) => {
                let name = self
                    .query_voyager_and_decode(voyager_api_url, contract_address)
                    .await;
                Some(ContractName { name, symbol: None })
            }
            None => None,
        }
    }

    pub async fn set_contract_names(&mut self, contract_calls_map: &mut ContractCallsMap) {
        self.collect_contract_addresses(contract_calls_map);
        self.token_contract_names = self.fetch_token_contract_names().await;
        if let Some(voyager_api_url) = &self.voyager_api_url {
            self.contract_names = self.fetch_contract_names(voyager_api_url.clone()).await;
        }
        self.update_simulation_call_trace(contract_calls_map);
    }

    fn collect_contract_addresses(&mut self, contract_calls_map: &mut ContractCallsMap) {
        for call in contract_calls_map.0.values() {
            if call.is_erc20_token {
                self.token_addresses
                    .insert(call.entry_point.storage_address);
            } else {
                self.contract_addresses
                    .insert(call.entry_point.storage_address);
            }
        }
    }

    async fn fetch_token_contract_names(&self) -> HashMap<ContractAddress, ContractName> {
        let name_method_selector: Felt = selector!("name").into();
        let symbol_method_selector: Felt = selector!("symbol").into();

        let futures = self.token_addresses.iter().map(|token_contract_address| {
            let contract_address_felt: Felt = (*token_contract_address).into();

            async move {
                let token_name_future =
                    self.query_contract_and_decode(contract_address_felt, name_method_selector);

                let token_symbol_future =
                    self.query_contract_and_decode(contract_address_felt, symbol_method_selector);

                let (token_name, token_symbol) =
                    futures::join!(token_name_future, token_symbol_future);

                (
                    *token_contract_address,
                    ContractName {
                        name: token_name,
                        symbol: token_symbol,
                    },
                )
            }
        });

        let results = join_all(futures).await;

        let contract_names: HashMap<ContractAddress, ContractName> = results.into_iter().collect();
        contract_names
    }

    async fn fetch_contract_names(
        &self,
        voyager_api_url: String,
    ) -> HashMap<ContractAddress, ContractName> {
        let futures = self.contract_addresses.iter().map(|contract_address| {
            let contract_address_string: String = (*contract_address).to_string();
            let voyager_api_url = voyager_api_url.clone();
            async move {
                let contract_name_future =
                    self.query_voyager_and_decode(&voyager_api_url, contract_address_string);
                let (contract_name,) = futures::join!(contract_name_future);
                (
                    *contract_address,
                    ContractName {
                        name: contract_name,
                        symbol: None,
                    },
                )
            }
        });

        let results = join_all(futures).await;

        let contract_names: HashMap<ContractAddress, ContractName> = results.into_iter().collect();
        contract_names
    }

    async fn query_contract_and_decode(
        &self,
        token_contract_address: Felt,
        entry_point_selector: Felt,
    ) -> Option<String> {
        let call_result = self
            .provider_client
            .call(
                starknet_old_types::FunctionCall {
                    contract_address: felt_to_field_element(token_contract_address),
                    entry_point_selector: felt_to_field_element(entry_point_selector),
                    calldata: vec![],
                },
                starknet_old_types::BlockId::Tag(starknet_old_types::BlockTag::Latest),
            )
            .await;
        if let Ok(call_result) = call_result {
            let name = bytes_to_text(call_result.first().unwrap().to_bytes_be());
            if let Ok(name) = name {
                return Some(name);
            }
        }
        None
    }

    async fn query_voyager_and_decode(
        &self,
        voyager_api_url: &str,
        contract_address: String,
    ) -> Option<String> {
        let client = reqwest::Client::new();
        let url = format!("{}contracts/{}", voyager_api_url, contract_address);
        let call_result = client
            .get(&url)
            .header("x-api-key", "Ji6ugSKp8L64EvevISdfb9CgY0sUBEhz6P4uPYOB")
            .send()
            .await;
        match call_result {
            Ok(response) => {
                if response.status().is_success() {
                    let contract_details: Value = response.json().await.unwrap();
                    let contract_alias: Option<String> =
                        match contract_details["contractAlias"].as_str() {
                            Some(alias) => Some(alias.to_string()),
                            None => contract_details["classAlias"]
                                .as_str()
                                .map(|inner_alias| inner_alias.to_string()),
                        };
                    contract_alias
                } else {
                    None
                }
            }
            Err(e) => {
                info!("Failed to fetch contract details from voyager api: {}", e);
                None
            }
        }
    }

    fn update_simulation_call_trace(&self, contract_calls_map: &mut ContractCallsMap) {
        for call in contract_calls_map.0.values_mut() {
            if call.is_erc20_token {
                if let Some(contract_name) = self
                    .token_contract_names
                    .get(&call.entry_point.storage_address)
                {
                    call.erc20_token_name = contract_name.name.clone();
                    call.erc20_token_symbol = contract_name.symbol.clone();
                }
            } else if let Some(contract_name) =
                self.contract_names.get(&call.entry_point.storage_address)
            {
                call.contract_name = contract_name.name.clone();
            }
        }
    }
}
