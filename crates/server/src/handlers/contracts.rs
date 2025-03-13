use crate::app_state::AppState;
use axum::{
    extract::{Path, Query, State},
    http::StatusCode,
    response::{IntoResponse, Response},
    Json,
};
use blockifier::abi::abi_utils::selector_from_name;
use serde::{Deserialize, Serialize};
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
use walnut_shared::abi::Function;
use walnut_shared::abi::Input;
use walnut_shared::abi::Item;
use walnut_shared::abi::Output;
use walnut_shared::utils::simplify_type_name;
use walnut_shared::{
    chain_id_to_readable_string, create_rpc_client, create_rpc_client_from_url, extract_chain_id,
    felt_to_field_element, field_element_to_felt,
};

#[derive(Serialize, ToSchema)]
pub struct ContractAbiResponse {
    pub entry_point_datas: Vec<(String, Function)>,
}

#[derive(Deserialize, Debug, Serialize, ToSchema)]
pub struct ContractAddressQuery {
    pub rpc_url: Option<String>,
    pub chain_id: Option<String>,
}

#[utoipa::path(
    get,
    path = "/v1/contracts/{contract_address}/entrypoints",
    responses(
        (status = 200, description = "Returns the list of entry points of the contract", body = ContractFunctionResponse),
        (status = 404, description = "Contract not found"),
        (status = 500, description = "Internal server error")
    ),
    params(
        ("contract_address" = String, Path, description = "Contract address"),
        ("rpc_urls" = Option<String>, Query, description = "Comma-separated list of additional RPC URLs to check")
    ),
    tag = "Contract entrypoints"
)]
pub async fn get_contract_entrypoints_handler(
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

    let provider = if let Some(chain_id) = query.chain_id {
        let chain_id = match extract_chain_id(chain_id.as_str()) {
            Ok(chain_id) => chain_id,
            Err(_) => return (StatusCode::BAD_REQUEST, "Invalid chain ID").into_response(),
        };
        create_rpc_client(&chain_id)
    } else if let Some(rpc_url) = query.rpc_url {
        let rpc_url = match Url::parse(&rpc_url) {
            Ok(url) => url,
            Err(_) => return (StatusCode::BAD_REQUEST, "Invalid RPC URL").into_response(),
        };
        create_rpc_client_from_url(rpc_url)
    } else {
        return (
            StatusCode::BAD_REQUEST,
            "Either chain_id or rpc_url must be provided",
        )
            .into_response();
    };

    let mut entry_point_datas: Vec<(String, Function)> = Vec::new();

    match provider
        .get_class_at(
            starknet_old_types::BlockId::Tag(starknet_old_types::BlockTag::Latest),
            contract_address_field,
        )
        .await
    {
        Ok(contract_class) => {
            if let Some(abi) = match contract_class {
                starknet_old_types::ContractClass::Sierra(sierra_class) => Some(sierra_class.abi),
                starknet_old_types::ContractClass::Legacy(_) => None,
            } {
                match serde_json::from_str::<Vec<Item>>(&abi) {
                    Ok(parsed_abi) => {
                        let functions: Vec<(String, Function)> = get_functions(&parsed_abi)
                            .iter()
                            .map(|func| {
                                let simplified_inputs: Vec<Input> = func
                                    .inputs
                                    .iter()
                                    .map(|input| Input {
                                        name: input.name.clone(),
                                        ty: simplify_type_name(&input.ty),
                                    })
                                    .collect();

                                let simplified_outputs: Vec<Output> = func
                                    .outputs
                                    .iter()
                                    .map(|output| Output {
                                        ty: simplify_type_name(&output.ty),
                                    })
                                    .collect();

                                (
                                    selector_from_name(&func.name).0.to_fixed_hex_string(),
                                    Function {
                                        name: func.name.clone(),
                                        inputs: simplified_inputs,
                                        outputs: simplified_outputs,
                                        state_mutability: func.state_mutability.clone(),
                                    },
                                )
                            })
                            .collect();

                        entry_point_datas.extend(functions);
                    }
                    Err(err) => {
                        error!("Failed to parse ABI for network: {}", err);
                    }
                }
            }
        }
        Err(err) => {
            error!(
                "Failed to fetch class for address {} from source: {}",
                contract_address, err
            );
        }
    }

    if entry_point_datas.is_empty() {
        return (StatusCode::NOT_FOUND, "ABI not found for contract address").into_response();
    }

    let response = ContractAbiResponse { entry_point_datas };

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
