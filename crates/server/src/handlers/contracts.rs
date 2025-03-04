use crate::app_state::AppState;
use crate::services::search::sources_from_rpc_urls;
use crate::services::search::{ESource, ESourceType};
use axum::{
    extract::{Path, Query, State},
    http::StatusCode,
    response::{IntoResponse, Response},
    Json,
};
use serde::{Deserialize, Serialize};
use starknet::core::types::ContractClass;
use starknet::core::types::Felt;
use starknet_api::core::ChainId;
use starknet_old::core::types as starknet_old_types;
use starknet_providers::{jsonrpc::HttpTransport, JsonRpcClient, Provider};
use std::str::FromStr;
use std::{collections::HashMap, sync::Arc};
use tracing::error;
use url::Url;
use utoipa::ToSchema;
use verification::{db::fetch_verified_class, s3::fetch_verified_class_hash_with_source_code_data};
use walnut_shared::abi::get_functions;
use walnut_shared::abi::Item;
use walnut_shared::{
    chain_id_to_readable_string, create_rpc_client, create_rpc_client_from_url,
    field_element_to_felt,
};
use walnut_shared::{extract_chain_id, felt_to_field_element};

#[derive(Serialize, ToSchema)]
pub struct ContractAbiResponse {
    pub functions: HashMap<String, Vec<String>>,
}

#[derive(Deserialize, Debug, Serialize, ToSchema)]
pub struct ContractAddressQuery {
    pub rpc_urls: Option<String>,
}

#[utoipa::path(
    get,
    path = "/v1/contracts/{contract_address}/functions",
    responses(
        (status = 200, description = "Returns the list of functions of the contract", body = ContractFunctionResponse),
        (status = 404, description = "Contract not found"),
        (status = 500, description = "Internal server error")
    ),
    params(
        ("contract_address" = String, Path, description = "Contract address"),
        ("rpc_urls" = Option<String>, Query, description = "Comma-separated list of additional RPC URLs to check")
    ),
    tag = "Contract functions"
)]
pub async fn get_contract_function_handler(
    State(_state): State<Arc<AppState>>,
    Path(contract_address): Path<String>,
    Query(query): Query<ContractAddressQuery>,
) -> Response {
    let contract_address_field = match Felt::from_hex(&contract_address) {
        Ok(felt) => felt_to_field_element(felt),
        Err(_) => {
            let error_message = "Invalid  contract address format";
            error!(error_message);
            return (StatusCode::BAD_REQUEST, Json(error_message)).into_response();
        }
    };

    let sources = match sources_from_rpc_urls(query.rpc_urls.as_deref()) {
        Ok(sources) => sources,
        Err(err) => {
            return (
                StatusCode::BAD_REQUEST,
                format!("Error parsing URLs: {}", err),
            )
                .into_response()
        }
    };

    let mut functions_map: HashMap<String, Vec<String>> = HashMap::new();

    for source in &sources {
        let (network, provider) = match source {
            ESourceType::ChainId(chain_id) => (
                chain_id_to_readable_string(&chain_id),
                create_rpc_client(&chain_id),
            ),
            ESourceType::RpcUrl(url) => (url.to_string(), create_rpc_client_from_url(url.clone())),
        };

        match provider
            .get_class_at(
                starknet_old_types::BlockId::Tag(starknet_old_types::BlockTag::Latest),
                contract_address_field,
            )
            .await
        {
            Ok(contract_class) => {
                if let Some(abi) = match contract_class {
                    starknet_old_types::ContractClass::Sierra(sierra_class) => {
                        Some(sierra_class.abi)
                    }
                    starknet_old_types::ContractClass::Legacy(_) => None,
                } {
                    match serde_json::from_str::<Vec<Item>>(&abi) {
                        Ok(parsed_abi) => {
                            let functions = get_functions(&parsed_abi)
                                .iter()
                                .map(|func| func.name.clone())
                                .collect();
                            functions_map.insert(network, functions);
                        }
                        Err(err) => {
                            error!("Failed to parse ABI for network {}: {}", network, err);
                        }
                    }
                }
            }
            Err(err) => {
                error!(
                    "Failed to fetch class for address {} from source {:?}: {}",
                    contract_address, source, err
                );
                functions_map.insert(network, vec![]);
            }
        }
    }

    if functions_map.is_empty() {
        return (StatusCode::NOT_FOUND, "ABI not found for contract address").into_response();
    }

    let response = ContractAbiResponse {
        functions: functions_map,
    };

    (StatusCode::OK, Json(response)).into_response()
}

#[derive(Deserialize, Debug, Serialize, ToSchema)]
pub struct ContractResponseWithSourceCode {
    pub chain_id: Option<String>,
    pub class_hash: String,
    pub is_class_verified: bool,
    pub source_code: Option<HashMap<String, String>>,
}

#[derive(Debug, Deserialize)]
pub struct ContractQueryParams {
    pub rpc_url: Option<String>,
    pub chain_id: Option<String>,
    pub include_source_code: Option<bool>,
}

#[utoipa::path(
    post,
    path = "/v1/contracts/{contract_address}",
    responses(
        (status = 200, description = "Returns the contract data", body = ContractResponseWithSourceCode),
        (status = 404, description = "Contract not found for contract address", body = String)
    ),
    params(
        ("contract_address" = String, Path, description = "Contract address"),
        ("rpc_url" = Option<String>, Query, description = "RPC URL"),
        ("chain_id" = Option<String>, Query, description = "Chain identifier"), 
        ("include_source_code" = Option<bool>, Query, description = "Whether to include the source code in the response")
    ),
    tag = "Contract details"
)]
pub async fn get_contract_handler(
    State(state): State<Arc<AppState>>,
    Path(contract_address): Path<String>,
    Query(query_params): Query<ContractQueryParams>,
) -> Response {
    let contract = if let Some(chain_id) = query_params.chain_id {
        let chain_id = match extract_chain_id(chain_id.as_str()) {
            Ok(chain_id) => chain_id,
            Err(_) => return (StatusCode::BAD_REQUEST, "Invalid chain ID").into_response(),
        };
        let provider_client = create_rpc_client(&chain_id);
        fetch_contract_data(
            &state,
            &contract_address,
            query_params.include_source_code.unwrap_or(false),
            provider_client,
            Some(&chain_id),
        )
        .await
    } else if let Some(rpc_url) = query_params.rpc_url {
        let rpc_url = match Url::parse(&rpc_url) {
            Ok(url) => url,
            Err(_) => return (StatusCode::BAD_REQUEST, "Invalid RPC URL").into_response(),
        };
        let provider_client = create_rpc_client_from_url(rpc_url);
        fetch_contract_data(
            &state,
            &contract_address,
            query_params.include_source_code.unwrap_or(false),
            provider_client,
            None,
        )
        .await
    } else {
        return (
            StatusCode::BAD_REQUEST,
            "Either chain_id or rpc_url must be provided",
        )
            .into_response();
    };

    if contract.is_none() {
        (StatusCode::NOT_FOUND, "Contract not found").into_response()
    } else {
        (StatusCode::OK, Json(Some(contract))).into_response()
    }
}

async fn fetch_contract_data(
    state: &Arc<AppState>,
    contract_address: &str,
    include_source_code: bool,
    provider_client: JsonRpcClient<HttpTransport>,
    chain_id: Option<&ChainId>,
) -> Option<ContractResponseWithSourceCode> {
    let class_hash = provider_client
        .get_class_hash_at(
            starknet_old_types::BlockId::Tag(starknet_old_types::BlockTag::Latest),
            felt_to_field_element(Felt::from_str(contract_address).ok()?),
        )
        .await
        .ok()?;
    let class_hash_str = field_element_to_felt(class_hash).to_fixed_hex_string();

    let is_verified = fetch_verified_class(&state.db_pool, &class_hash_str)
        .await
        .is_ok();

    let source_code = if is_verified && include_source_code {
        match fetch_verified_class_hash_with_source_code_data(
            &state.db_pool,
            &state.s3_client,
            &class_hash_str,
        )
        .await
        {
            Ok(source_code) => source_code,
            Err(_) => None,
        }
    } else {
        None
    };

    Some(ContractResponseWithSourceCode {
        chain_id: chain_id.map(chain_id_to_readable_string),
        class_hash: class_hash_str,
        is_class_verified: is_verified,
        source_code,
    })
}
