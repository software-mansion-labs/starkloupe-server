use crate::app_state::AppState;
use crate::services::search::{sources_from_rpc_urls, ESource, ESourceType};
use axum::{
    extract::{Path, Query, State},
    http::StatusCode,
    response::{IntoResponse, Response},
    Json,
};
use blockifier::abi::abi_utils::selector_from_name;
use futures::stream::FuturesUnordered;
use futures::StreamExt;
use serde::{Deserialize, Serialize};
use starknet::core::types::Felt;
use starknet_api::core::ChainId;
use starknet_old::core::types as starknet_old_types;
use starknet_providers::Provider;
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
    chain_id_to_readable_string, create_rpc_client, create_rpc_client_from_url,
    extract_cairo_version_from_program, extract_chain_id, felt_to_field_element,
    tuple_to_version_string, EChainId, ENetwork,
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
        let (e_chain_id, network) = match extract_chain_id(chain_id.as_str()) {
            Ok(chain_id) => chain_id,
            Err(_) => return (StatusCode::BAD_REQUEST, "Invalid chain ID").into_response(),
        };
        if network == ENetwork::Ethereum {
            let error_message = "Ethereum network is not supported for verification";
            error!(chain_id = chain_id.as_str(), "{}", error_message);
            return (StatusCode::BAD_REQUEST, error_message).into_response();
        }

        let core_chain_id = ChainId::from(e_chain_id);
        create_rpc_client(&core_chain_id)
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
pub struct GetContractResponse {
    pub class_hash: String,
    pub verified: bool,
    pub deployed_sources: Vec<ESource>,
    pub cairo_version: String,
    pub source_code: Option<HashMap<String, String>>,
}

#[derive(Debug, Deserialize)]
pub struct ContractQuery {
    pub rpc_urls: Option<String>,
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
        ("rpc_urls" = Option<String>, Query, description = "Comma-separated list of additional RPC URLs to check"),
        ("include_source_code" = Option<bool>, Query, description = "Whether to include the source code in the response")
    ),
    tag = "Contract details"
)]
pub async fn get_contract_handler(
    State(state): State<Arc<AppState>>,
    Path(contract_address): Path<String>,
    Query(query): Query<ContractQuery>,
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

    let results: Vec<Option<(ESource, Felt, (u32, u32, u32))>> = sources
        .iter()
        .filter_map(|source| {
            if let ESourceType::ChainId(e_chain_id) = source {
                match e_chain_id {
                    EChainId::StarknetMainnet | EChainId::StarknetSepolia => (),
                    _ => return None,
                }
            }
            Some(async move {
                let provider = match source {
                    ESourceType::ChainId(chain_id) => {
                        let core_chain_id = ChainId::from(*chain_id);
                        create_rpc_client(&core_chain_id)
                    }
                    ESourceType::RpcUrl(url) => create_rpc_client_from_url(url.clone()),
                };

                if let Ok(class_from_blockchain) = provider
                    .get_class_at(
                        starknet_old_types::BlockId::Tag(starknet_old_types::BlockTag::Latest),
                        contract_address_field,
                    )
                    .await
                {
                    let class_json = serde_json::to_value(&class_from_blockchain).ok()?;
                    let contract_class: starknet::core::types::ContractClass =
                        serde_json::from_value(class_json).ok()?;

                    if let starknet::core::types::ContractClass::Sierra(flattened_sierra_class) =
                        contract_class
                    {
                        let class_hash_felt = flattened_sierra_class.class_hash();
                        let cairo_version = extract_cairo_version_from_program(
                            &flattened_sierra_class.sierra_program,
                        )
                        .ok()?;
                        return Some((source.into(), class_hash_felt, cairo_version));
                    }
                }
                None
            })
        })
        .collect::<FuturesUnordered<_>>()
        .collect::<Vec<_>>()
        .await;

    let valid_results: Vec<(ESource, Felt, (u32, u32, u32))> =
        results.into_iter().flatten().collect();

    if valid_results.is_empty() {
        let network_list: Vec<String> = sources
            .iter()
            .filter_map(|source| match source {
                ESourceType::ChainId(chain_id) => {
                    if let EChainId::StarknetMainnet | EChainId::StarknetSepolia = chain_id {
                        Some(chain_id_to_readable_string(&ChainId::from(*chain_id)))
                    } else {
                        None
                    }
                }
                ESourceType::RpcUrl(_) => Some("custom RPC".to_string()),
            })
            .collect();

        return (
            StatusCode::NOT_FOUND,
            format!(
                "Contract not found on Starknet networks and custom RPCs: {}",
                network_list.join(", ")
            ),
        )
            .into_response();
    }

    let (_, class_hash_felt, cairo_version_tuple) = valid_results[0];
    let cairo_version_str = tuple_to_version_string(cairo_version_tuple);

    let valid_sources: Vec<ESource> = valid_results.into_iter().map(|(s, _, _)| s).collect();

    let class_hash = class_hash_felt.to_fixed_hex_string();
    let is_verified = fetch_verified_class(&state.db_pool, &class_hash)
        .await
        .is_ok();

    let source_code = if is_verified && query.include_source_code.unwrap_or_default() {
        match fetch_verified_class_hash_with_source_code_data(
            &state.db_pool,
            &state.s3_client,
            &class_hash,
        )
        .await
        {
            Ok(source_code) => source_code,
            Err(_) => None,
        }
    } else {
        None
    };

    let response_body = GetContractResponse {
        class_hash,
        verified: is_verified,
        deployed_sources: valid_sources,
        cairo_version: cairo_version_str,
        source_code,
    };

    (StatusCode::OK, Json(response_body)).into_response()
}
