use crate::app_state::AppState;
use crate::services::search::{sources_from_rpc_urls, ESource, ESourceType};
use axum::{
    extract::{Path, Query, State},
    http::StatusCode,
    response::{IntoResponse, Response},
    Json,
};
use data_decoder::type_decoder::{
    expand_enums_recursively, expand_structs_recursively, TypeDecoder,
};
use futures::stream::FuturesUnordered;
use futures::StreamExt;
use serde::{Deserialize, Serialize};
use starknet_api::core::ChainId;
use starknet_rust::core::types::{BlockId, BlockTag, ContractClass, Felt};
use starknet_rust::providers::Provider;
use std::{collections::HashMap, sync::Arc};
use tracing::error;
use url::Url;
use utoipa::ToSchema;
use verification::{
    db::{build_class_sources, fetch_verified_class},
    gcs::fetch_verified_class_hash_with_source_code_data,
};
use walnut_shared::abi::{get_enums, get_functions, get_structs, Function, Item};
use walnut_shared::utils::simplify_type_name;
use walnut_shared::{
    chain_id_to_readable_string, create_rpc_client, create_rpc_client_from_url,
    extract_cairo_version_from_program, extract_chain_id, fetch_voyager_contract_alias,
    get_voyager_api_url, tuple_to_version_string, EChainId, ENetwork,
};

#[derive(Serialize, ToSchema)]
pub struct ContractAbiResponse {
    pub entry_point_datas: Vec<(String, Function)>,
}

// One class found on a source: (source, class hash, cairo version, optional ABI).
type ClassLookup = (ESource, Felt, (u32, u32, u32), Option<String>);

#[derive(Deserialize, Debug, Serialize, ToSchema)]
pub struct ContractAddressQuery {
    pub rpc_url: Option<String>,
    pub chain_id: Option<String>,
}

#[utoipa::path(
    get,
    path = "/v1/contracts/{contract_address}/entrypoints",
    responses(
        (status = 200, description = "Returns the list of entry points of the contract", body = ContractAbiResponse),
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
    let contract_address_felt = match Felt::from_hex(&contract_address) {
        Ok(felt) => felt,
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
            starknet_rust::core::types::BlockId::Tag(starknet_rust::core::types::BlockTag::Latest),
            contract_address_felt,
        )
        .await
    {
        Ok(contract_class) => {
            if let Some(abi) = match contract_class {
                ContractClass::Sierra(sierra_class) => Some(sierra_class.abi),
                ContractClass::Legacy(_) => None,
            } {
                match serde_json::from_str::<Vec<Item>>(&abi) {
                    Ok(parsed_abi) => {
                        // Pre-extract structs and enums once, create lookup maps
                        let structs = get_structs(&parsed_abi);
                        let enums = get_enums(&parsed_abi);

                        // Found structs and enums in ABI
                        let struct_map: std::collections::HashMap<
                            String,
                            walnut_shared::abi::Struct,
                        > = structs
                            .into_iter()
                            .map(|s| (simplify_type_name(&s.name), s))
                            .collect();
                        let enum_map: std::collections::HashMap<String, walnut_shared::abi::Enum> =
                            enums
                                .into_iter()
                                .map(|e| (simplify_type_name(&e.name), e))
                                .collect();

                        // Recursively enhance all structs and enums with their members/variants
                        let expanded_struct_map =
                            expand_structs_recursively(&struct_map, &enum_map);
                        let expanded_enum_map =
                            expand_enums_recursively(&enum_map, &expanded_struct_map);

                        let functions = get_functions(&parsed_abi);

                        // Use TypeDecoder to decode functions with complete type information
                        let type_decoder = TypeDecoder::new(expanded_struct_map, expanded_enum_map);
                        entry_point_datas = type_decoder.decode_functions(&functions);
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
    pub sources: Vec<String>,
    pub abi: Option<String>,
    pub contract_name: Option<String>,
    pub class_name: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct ContractQuery {
    pub rpc_urls: Option<String>,
    pub include_source_code: Option<bool>,
    pub include_abi: Option<bool>,
}

#[utoipa::path(
    post,
    path = "/v1/contracts/{contract_address}",
    responses(
        (status = 200, description = "Returns the contract data", body = GetContractResponse),
        (status = 404, description = "Contract not found for contract address", body = String)
    ),
    params(
        ("contract_address" = String, Path, description = "Contract address"),
        ("rpc_urls" = Option<String>, Query, description = "Comma-separated list of additional RPC URLs to check"),
        ("include_source_code" = Option<bool>, Query, description = "Whether to include the source code in the response"),
        ("include_abi" = Option<bool>, Query, description = "Whether to include the ABI in the response")
    ),
    tag = "Contract details"
)]

pub async fn get_contract_handler(
    State(state): State<Arc<AppState>>,
    Path(contract_address): Path<String>,
    Query(query): Query<ContractQuery>,
) -> Response {
    let contract_address_felt = match Felt::from_hex(&contract_address) {
        Ok(felt) => felt,
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

    let include_abi = query.include_abi.unwrap_or_default();

    let results: Vec<Option<ClassLookup>> = sources
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
                    .get_class_at(BlockId::Tag(BlockTag::Latest), contract_address_felt)
                    .await
                {
                    let class_json = serde_json::to_value(&class_from_blockchain).ok()?;
                    let contract_class: ContractClass = serde_json::from_value(class_json).ok()?;

                    if let ContractClass::Sierra(flattened_sierra_class) = contract_class {
                        let class_hash_felt = flattened_sierra_class.class_hash();
                        let cairo_version = extract_cairo_version_from_program(
                            &flattened_sierra_class.sierra_program,
                        )
                        .ok()?;
                        let abi = if include_abi {
                            Some(flattened_sierra_class.abi.clone())
                        } else {
                            None
                        };
                        return Some((source.into(), class_hash_felt, cairo_version, abi));
                    }
                }
                None
            })
        })
        .collect::<FuturesUnordered<_>>()
        .collect::<Vec<_>>()
        .await;

    let valid_results: Vec<ClassLookup> = results.into_iter().flatten().collect();

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

    let first_result = &valid_results[0];
    let class_hash_felt = first_result.1;
    let cairo_version_tuple = first_result.2;
    let abi = first_result.3.clone();
    let cairo_version_str = tuple_to_version_string(cairo_version_tuple);

    let valid_sources: Vec<ESource> = valid_results.into_iter().map(|(s, _, _, _)| s).collect();

    let class_hash = class_hash_felt.to_fixed_hex_string();
    let is_verified_locally = fetch_verified_class(&state.db_pool, &class_hash)
        .await
        .is_ok();

    let (source_code, voyager_hit, is_verified, class_name) =
        if query.include_source_code.unwrap_or_default() {
            if is_verified_locally {
                // Try local (Walnut) first
                match fetch_verified_class_hash_with_source_code_data(
                    &state.db_pool,
                    &state.gcs_client,
                    &class_hash,
                )
                .await
                {
                    Ok(Some(code)) => (Some(code), false, true, None),
                    _ => {
                        // Fallback to Voyager
                        if let Some(voyager_client) = &state.voyager_client {
                            match voyager_client.fetch_source_code(&class_hash).await {
                                Ok(Some(voyager_response)) => (
                                    Some(voyager_response.source_code),
                                    true,
                                    true,
                                    Some(voyager_response.verified_name),
                                ),
                                _ => (None, false, true, None), // Still verified locally even if no source code
                            }
                        } else {
                            (None, false, true, None)
                        }
                    }
                }
            } else {
                // Not verified locally, try Voyager
                if let Some(voyager_client) = &state.voyager_client {
                    match voyager_client.fetch_source_code(&class_hash).await {
                        Ok(Some(voyager_response)) => (
                            Some(voyager_response.source_code),
                            true,
                            true,
                            Some(voyager_response.verified_name),
                        ),
                        _ => (None, false, false, None),
                    }
                } else {
                    (None, false, false, None)
                }
            }
        } else {
            // No source code requested, check Voyager for verification status
            if is_verified_locally {
                (None, false, true, None)
            } else if let Some(voyager_client) = &state.voyager_client {
                match voyager_client.fetch_source_code(&class_hash).await {
                    Ok(Some(voyager_response)) => {
                        (None, true, true, Some(voyager_response.verified_name))
                    }
                    _ => (None, false, false, None),
                }
            } else {
                (None, false, false, None)
            }
        };

    // Look up the deployed contract's alias from Voyager. We try the Starknet
    // chains where the contract was actually found, and stop at the first hit.
    let mut contract_name: Option<String> = None;
    for source in &valid_sources {
        if let ESource::ChainId(chain_str) = source {
            let chain_id_for_voyager = match chain_str.as_str() {
                "sn_mainnet" => Some(ChainId::Mainnet),
                "sn_sepolia" => Some(ChainId::Sepolia),
                _ => None,
            };
            if let Some(chain_id) = chain_id_for_voyager {
                if let Some(voyager_url) = get_voyager_api_url(&chain_id) {
                    if let Some(alias) =
                        fetch_voyager_contract_alias(voyager_url, &contract_address).await
                    {
                        contract_name = Some(alias);
                        break;
                    }
                }
            }
        }
    }

    let sources = build_class_sources(
        &state.db_pool,
        &class_hash,
        is_verified_locally,
        voyager_hit,
    )
    .await;

    let response_body = GetContractResponse {
        class_hash,
        verified: is_verified,
        deployed_sources: valid_sources,
        cairo_version: cairo_version_str,
        source_code,
        sources,
        abi,
        contract_name,
        class_name,
    };

    (StatusCode::OK, Json(response_body)).into_response()
}
