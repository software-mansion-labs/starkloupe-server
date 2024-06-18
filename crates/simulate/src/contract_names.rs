use crate::EventTrace;
use crate::SimulationCallTrace;
use futures::future::join_all;
use serde_json::Value;
use starknet::{
    core::types::{BlockId, BlockTag, FieldElement, FunctionCall},
    macros::selector,
    providers::{jsonrpc::HttpTransport, JsonRpcClient, Provider},
};
use starknet_api::{
    core::{ChainId, ContractAddress},
    hash::StarkFelt,
};
use std::collections::{HashMap, HashSet};
use walnut_shared::{bytes_to_text, create_rpc_client, voyager_api_url};

pub struct ContractNamesFetcher {
    provider_client: JsonRpcClient<HttpTransport>,
    voyager_api: String,
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
    pub fn new(chain_id: &ChainId) -> Self {
        let provider_client = create_rpc_client(chain_id);
        let voyager_api = voyager_api_url(chain_id).to_string();
        ContractNamesFetcher {
            provider_client,
            voyager_api,
            contract_addresses: HashSet::new(),
            token_addresses: HashSet::new(),
            token_contract_names: HashMap::new(),
            contract_names: HashMap::new(),
        }
    }

    pub async fn enhance_trace_with_contract_names(
        &mut self,
        simulation_call_trace: &mut SimulationCallTrace,
        event_traces: &mut Vec<EventTrace>,
    ) {
        self.get_contract_addresses(simulation_call_trace);
        self.token_contract_names = self.fetch_token_contract_names().await;
        self.contract_names = self.fetch_contract_names().await;
        self.update_simulation_call_trace(simulation_call_trace);
        for event_trace in &mut event_traces.iter_mut() {
            self.update_event_trace_with_contract_names(simulation_call_trace, event_trace);
        }
    }

    fn get_contract_addresses(&mut self, simulation_call_trace: &SimulationCallTrace) {
        if simulation_call_trace.additional_info.is_erc20_token {
            self.token_addresses
                .insert(simulation_call_trace.entry_point.storage_address);
        } else {
            self.contract_addresses
                .insert(simulation_call_trace.entry_point.storage_address);
        }
        for nested_call in &simulation_call_trace.nested_calls {
            self.get_contract_addresses(nested_call);
        }
    }

    async fn fetch_token_contract_names(&self) -> HashMap<ContractAddress, ContractName> {
        let name_method_selector: FieldElement = selector!("name").into();
        let symbol_method_selector: FieldElement = selector!("symbol").into();

        let futures = self.token_addresses.iter().map(|token_contract_address| {
            let contract_address_felt: StarkFelt = (*token_contract_address).into();
            let contract_address_field_element: FieldElement = contract_address_felt.into();

            async move {
                let token_name_future = self.query_contract_and_decode(
                    contract_address_field_element,
                    name_method_selector,
                );

                let token_symbol_future = self.query_contract_and_decode(
                    contract_address_field_element,
                    symbol_method_selector,
                );

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

    async fn fetch_contract_names(&self) -> HashMap<ContractAddress, ContractName> {
        let futures = self.contract_addresses.iter().map(|contract_address| {
            let contract_address_felt: StarkFelt = (*contract_address).into();
            let contract_address_string: String = contract_address_felt.to_string();
            async move {
                let contract_name_future = self.query_voyager_and_decode(contract_address_string);
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
        token_contract_address: FieldElement,
        entry_point_selector: FieldElement,
    ) -> Option<String> {
        let call_result = self
            .provider_client
            .call(
                FunctionCall {
                    contract_address: token_contract_address,
                    entry_point_selector,
                    calldata: vec![],
                },
                BlockId::Tag(BlockTag::Latest),
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

    async fn query_voyager_and_decode(&self, contract_address: String) -> Option<String> {
        let client = reqwest::Client::new();
        let url = format!("{}contracts/{}", self.voyager_api, contract_address);
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
                println!("Failed to fetch contract details from voyager api: {}", e);
                None
            }
        }
    }

    fn update_simulation_call_trace(&self, simulation_call_trace: &mut SimulationCallTrace) {
        if let Some(contract_name) = self
            .token_contract_names
            .get(&simulation_call_trace.entry_point.storage_address)
        {
            simulation_call_trace.additional_info.erc20_token_name = contract_name.name.clone();
            simulation_call_trace.additional_info.erc20_token_symbol = contract_name.symbol.clone();
        }

        if let Some(contract_name) = self
            .contract_names
            .get(&simulation_call_trace.entry_point.storage_address)
        {
            simulation_call_trace.additional_info.contract_name = contract_name.name.clone();
        }
        for nested_call in &mut simulation_call_trace.nested_calls {
            self.update_simulation_call_trace(nested_call);
        }
    }

    fn update_event_trace_with_contract_names(
        &self,
        simulation_call_trace: &SimulationCallTrace,
        event_trace: &mut EventTrace,
    ) {
        let storage_address_formatted = format!(
            "0x{:0>64}",
            simulation_call_trace
                .entry_point
                .storage_address
                .0
                .to_string()
                .trim_start_matches("0x")
        );
        if storage_address_formatted == event_trace.contract_name {
            let contract_name = simulation_call_trace.additional_info.contract_name.clone();
            match contract_name {
                Some(name) => {
                    event_trace.contract_name = name;
                }
                None => {
                    let token_name = simulation_call_trace
                        .additional_info
                        .erc20_token_name
                        .clone()
                        .unwrap_or_default();
                    let token_symbol = simulation_call_trace
                        .additional_info
                        .erc20_token_symbol
                        .clone()
                        .unwrap_or_default();
                    if !token_name.is_empty() && !token_symbol.is_empty() {
                        event_trace.contract_name = format!("{} ({})", token_name, token_symbol);
                    }
                }
            }
        }

        for nested_call in &simulation_call_trace.nested_calls {
            self.update_event_trace_with_contract_names(nested_call, event_trace)
        }
    }
}
