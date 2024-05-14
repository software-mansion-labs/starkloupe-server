use crate::SimulationCallTrace;
use futures::future::join_all;
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
use walnut_shared::{bytes_to_text, create_rpc_client};

pub struct ContractNamesFetcher {
    provider_client: JsonRpcClient<HttpTransport>,
    pub contract_addresses: HashSet<ContractAddress>,
    pub token_addresses: HashSet<ContractAddress>,
    pub contract_names: HashMap<ContractAddress, ContractName>,
}

#[derive(Debug)]
pub struct ContractName {
    pub token_name: Option<String>,
    pub token_symbol: Option<String>,
}

impl ContractNamesFetcher {
    pub fn new(chain_id: &ChainId) -> Self {
        let provider_client = create_rpc_client(chain_id);
        ContractNamesFetcher {
            provider_client,
            contract_addresses: HashSet::new(),
            token_addresses: HashSet::new(),
            contract_names: HashMap::new(),
        }
    }

    pub async fn enhance_trace_with_contract_names(
        &mut self,
        simulation_call_trace: &mut SimulationCallTrace,
    ) {
        self.get_contract_addresses(simulation_call_trace);
        self.contract_names = self.fetch_contract_names().await;
        self.update_simulation_call_trace(simulation_call_trace);
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

    async fn fetch_contract_names(&self) -> HashMap<ContractAddress, ContractName> {
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
                        token_name,
                        token_symbol,
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

    fn update_simulation_call_trace(&self, simulation_call_trace: &mut SimulationCallTrace) {
        if let Some(contract_name) = self
            .contract_names
            .get(&simulation_call_trace.entry_point.storage_address)
        {
            simulation_call_trace.additional_info.erc20_token_name =
                contract_name.token_name.clone();
            simulation_call_trace.additional_info.erc20_token_symbol =
                contract_name.token_symbol.clone();
        }
        for nested_call in &mut simulation_call_trace.nested_calls {
            self.update_simulation_call_trace(nested_call);
        }
    }
}
