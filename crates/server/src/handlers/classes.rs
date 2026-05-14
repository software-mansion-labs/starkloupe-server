use crate::{
    app_state::AppState,
    services::search::{check_class, sources_from_rpc_urls, ESource},
};
use axum::{
    extract::{Path, Query, State},
    http::StatusCode,
    response::{IntoResponse, Response},
    Json,
};
use chrono;
use reqwest::Client;
use serde::{Deserialize, Serialize};
use starknet_api::core::ChainId;
use starknet_rust::core::types::Felt;
use std::{collections::HashMap, sync::Arc};
use tracing::{error, warn};
use utoipa::ToSchema;
use verification::{
    db::fetch_verified_class,
    s3::{fetch_verified_class_hash_with_source_code_data, fetch_verified_class_with_data},
};
use walnut_shared::{get_voyager_api_url, VOYAGER_API_KEY};

#[derive(Deserialize, Debug, Serialize, ToSchema)]
pub struct GetClassResponseWithSourceCode {
    pub source_code: HashMap<String, String>,
    pub source: String,
}

#[utoipa::path(
    post,
    path = "/v1/{chain_id}/classes/{class_hash}",
    responses(
        (status = 200, description = "Returns the verified contract class data", body = GetClassResponseWithSourceCode),
        (status = 404, description = "Contract class not found for the given chain_id and class_hash", body = String)
    ),
    params(
        ("chain_id" = ChainId, Path, description = "Chain identifier"),
        ("class_hash" = String, Path, description = "Contract class hash"),
    ),
    tag = "Contract class verification"
)]
pub async fn get_class_handler_with_chain_id(
    State(state): State<Arc<AppState>>,
    Path((_chain_id, class_hash)): Path<(String, String)>,
) -> Response {
    // Try to fetch from local (Walnut) first
    if let Ok((_verified_class_row, verified_class_data)) =
        fetch_verified_class_with_data(&state.db_pool, &state.s3_client, &class_hash).await
    {
        return (
            StatusCode::OK,
            Json(GetClassResponseWithSourceCode {
                source_code: verified_class_data.source_code.clone(),
                source: "walnut".to_string(),
            }),
        )
            .into_response();
    }

    // Fallback to Voyager
    if let Some(voyager_client) = &state.voyager_client {
        match voyager_client.fetch_source_code(&class_hash).await {
            Ok(Some(voyager_response)) => {
                return (
                    StatusCode::OK,
                    Json(GetClassResponseWithSourceCode {
                        source_code: voyager_response.source_code,
                        source: "voyager".to_string(),
                    }),
                )
                    .into_response();
            }
            Ok(None) | Err(_) => {
                warn!("Class {} not found on Voyager", class_hash);
            }
        }
    }

    (
        StatusCode::NOT_FOUND,
        "Contract class is not verified or does not exist",
    )
        .into_response()
}

#[derive(Deserialize, Debug, Serialize, ToSchema)]
pub struct GetClassResponse {
    pub verified: bool,
    pub declared_sources: Vec<ESource>,
    pub source_code: Option<HashMap<String, String>>,
    pub source: Option<String>,
    pub class_name: Option<String>,
}

#[derive(Deserialize, Debug, Serialize, ToSchema)]
pub struct ClassQuery {
    pub rpc_urls: Option<String>,
    pub include_source_code: Option<bool>,
}

#[utoipa::path(
    post,
    path = "/v1/classes/{class_hash}",
    responses(
        (status = 200, description = "Returns if the contract class is verified or not", body = GetClassResponse),
    ),
    params(
        ("class_hash" = String, Path, description = "Contract class hash"),
        ("include_source_code" = Option<bool>, Query, description = "Whether to include the source code in the response"),
        ("rpc_urls" = Option<String>, Query, description = "Comma-separated list of additional RPC URLs to check")

    ),
    tag = "Contract class details"
)]
pub async fn get_class_handler(
    State(state): State<Arc<AppState>>,
    Path(class_hash): Path<String>,
    Query(query): Query<ClassQuery>,
) -> Response {
    let class_hash_fixed = match Felt::from_hex(&class_hash) {
        Ok(felt) => felt.to_fixed_hex_string(),
        Err(_) => {
            let error_message = "Invalid class hash format";
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

    let declared_sources = check_class(&class_hash_fixed, &sources)
        .await
        .unwrap_or_default()
        .into_iter()
        .map(|data| data.source)
        .collect();

    let is_verified_locally = fetch_verified_class(&state.db_pool, &class_hash_fixed)
        .await
        .is_ok();

    let (source_code, source, is_verified, class_name) = if query
        .include_source_code
        .unwrap_or(false)
    {
        if is_verified_locally {
            // Try local (Walnut) first
            match fetch_verified_class_hash_with_source_code_data(
                &state.db_pool,
                &state.s3_client,
                &class_hash_fixed,
            )
            .await
            {
                Ok(Some(code)) => (Some(code), Some("walnut".to_string()), true, None),
                _ => {
                    // Fallback to Voyager
                    if let Some(voyager_client) = &state.voyager_client {
                        match voyager_client.fetch_source_code(&class_hash_fixed).await {
                            Ok(Some(voyager_response)) => (
                                Some(voyager_response.source_code),
                                Some("voyager".to_string()),
                                true,
                                Some(voyager_response.verified_name),
                            ),
                            _ => (None, None, true, None), // Still verified locally even if no source code
                        }
                    } else {
                        (None, None, true, None)
                    }
                }
            }
        } else {
            // Not verified locally, try Voyager
            if let Some(voyager_client) = &state.voyager_client {
                match voyager_client.fetch_source_code(&class_hash_fixed).await {
                    Ok(Some(voyager_response)) => (
                        Some(voyager_response.source_code),
                        Some("voyager".to_string()),
                        true,
                        Some(voyager_response.verified_name),
                    ),
                    _ => (None, None, false, None),
                }
            } else {
                (None, None, false, None)
            }
        }
    } else {
        // No source code requested, check Voyager for verification status
        if is_verified_locally {
            (None, None, true, None)
        } else if let Some(voyager_client) = &state.voyager_client {
            match voyager_client.fetch_source_code(&class_hash_fixed).await {
                Ok(Some(voyager_response)) => (
                    None,
                    Some("voyager".to_string()),
                    true,
                    Some(voyager_response.verified_name),
                ),
                _ => (None, None, false, None),
            }
        } else {
            (None, None, false, None)
        }
    };

    (
        StatusCode::OK,
        Json(GetClassResponse {
            verified: is_verified,
            declared_sources,
            source_code,
            source,
            class_name,
        }),
    )
        .into_response()
}

#[derive(Deserialize, Debug, Serialize, ToSchema)]
pub struct GetContractsByClassHashResponse {
    pub class_hash: String,
    pub total_count: usize,
    pub contracts: Vec<ContractInfo>,
}

#[derive(Deserialize, Debug, Serialize, ToSchema)]
pub struct ContractInfo {
    pub address: String,
    pub deployment_time: Option<String>,
    pub networks: Vec<ESource>,
}

#[utoipa::path(
    get,
    path = "/v1/classes/{class_hash}/contracts",
    responses(
        (status = 200, description = "Returns all contracts with the given class hash", body = GetContractsByClassHashResponse),
        (status = 400, description = "Invalid class hash format or chain ID"),
        (status = 404, description = "No contracts found for the given class hash"),
        (status = 500, description = "Internal server error")
    ),
    params(
        ("class_hash" = String, Path, description = "Class hash"),
        ("rpc_urls" = Option<String>, Query, description = "Comma-separated list of additional RPC URLs to check")
    ),
    tag = "Contract class verification"
)]
pub async fn get_contracts_by_class_hash_handler(
    State(_state): State<Arc<AppState>>,
    Path(class_hash): Path<String>,
    Query(_query): Query<HashMap<String, String>>,
) -> Response {
    // Validate class hash format
    if !class_hash.starts_with("0x") {
        let error_message = "Invalid class hash format. Must start with 0x";
        return (StatusCode::BAD_REQUEST, Json(error_message)).into_response();
    }

    // Normalize to 66 characters (64 hex chars + "0x") by padding with leading zeros
    let hex_part = &class_hash[2..];
    let normalized_class_hash = if class_hash.len() < 66 {
        let padding_needed = 66 - class_hash.len();
        format!("0x{}{}", "0".repeat(padding_needed), hex_part)
    } else {
        class_hash.clone()
    };

    let class_hash_fixed = match Felt::from_hex(&normalized_class_hash) {
        Ok(felt) => felt.to_fixed_hex_string(),
        Err(_) => {
            let error_message = "Invalid class hash format";
            return (StatusCode::BAD_REQUEST, Json(error_message)).into_response();
        }
    };

    let sources = match sources_from_rpc_urls(_query.get("rpc_urls").map(|s| s.as_str())) {
        Ok(sources) => sources,
        Err(err) => {
            return (
                StatusCode::BAD_REQUEST,
                format!("Error parsing URLs: {}", err),
            )
                .into_response()
        }
    };

    // Find all networks where this class hash exists
    let declared_sources = check_class(&class_hash_fixed, &sources)
        .await
        .unwrap_or_default()
        .into_iter()
        .map(|data| data.source)
        .collect::<Vec<ESource>>();

    if declared_sources.is_empty() {
        return (
            StatusCode::NOT_FOUND,
            Json("Class hash not found on any supported networks".to_string()),
        )
            .into_response();
    }

    // Get contracts from all networks where class hash exists
    let mut all_contracts: Vec<ContractInfo> = Vec::new();
    let mut contract_networks: std::collections::HashMap<String, Vec<ESource>> =
        std::collections::HashMap::new();

    for source in &declared_sources {
        let chain_id = match source {
            ESource::ChainId(chain_name) => match chain_name.as_str() {
                "sn_mainnet" => ChainId::Mainnet,
                "sn_sepolia" => ChainId::Sepolia,
                _ => continue,
            },
            ESource::RpcUrl(_) => continue, // Skip custom RPCs for now
        };

        match get_contracts_from_voyager(&chain_id, &class_hash_fixed).await {
            Ok(contracts) => {
                for contract in contracts {
                    let address = contract.address.clone();
                    contract_networks
                        .entry(address.clone())
                        .or_insert_with(Vec::new)
                        .push(source.clone());

                    // Only add contract once (avoid duplicates)
                    if !all_contracts
                        .iter()
                        .any(|c: &ContractInfo| c.address == address)
                    {
                        all_contracts.push(ContractInfo {
                            address: contract.address,
                            deployment_time: contract.deployment_time,
                            networks: vec![source.clone()],
                        });
                    } else {
                        // Update networks for existing contract
                        if let Some(existing_contract) =
                            all_contracts.iter_mut().find(|c| c.address == address)
                        {
                            existing_contract.networks.push(source.clone());
                        }
                    }
                }
            }
            Err(e) => {
                warn!("Failed to fetch contracts from {:?}: {}", source, e);
                // Continue with other networks
            }
        }
    }

    if all_contracts.is_empty() {
        return (
            StatusCode::NOT_FOUND,
            Json("No contracts found for the given class hash".to_string()),
        )
            .into_response();
    }

    let response_body = GetContractsByClassHashResponse {
        class_hash: class_hash_fixed.clone(),
        total_count: all_contracts.len(),
        contracts: all_contracts,
    };

    (StatusCode::OK, Json(response_body)).into_response()
}

async fn get_contracts_from_voyager(
    chain_id: &ChainId,
    class_hash: &str,
) -> Result<Vec<ContractInfo>, Box<dyn std::error::Error>> {
    let voyager_url =
        get_voyager_api_url(chain_id).ok_or_else(|| "Unsupported chain for Voyager API")?;

    let url = format!("{}classes/{}/contracts", voyager_url, class_hash);

    let client = Client::new();
    let response = client
        .get(&url)
        .header("x-api-key", VOYAGER_API_KEY)
        .header("accept", "application/json")
        .send()
        .await?;

    if !response.status().is_success() {
        return Err(format!(
            "Voyager API error: {} - {}",
            response.status(),
            response.text().await.unwrap_or_default()
        )
        .into());
    }

    let json: serde_json::Value = response.json().await?;

    // Parse the response based on Voyager API structure
    let items = json["items"]
        .as_array()
        .unwrap_or(&vec![])
        .iter()
        .filter_map(|item| {
            Some(ContractInfo {
                address: item["address"].as_str()?.to_string(),
                deployment_time: item["creationTimestamp"].as_u64().map(|ts| {
                    // Convert timestamp to ISO string
                    chrono::DateTime::from_timestamp(ts as i64, 0)
                        .unwrap_or_default()
                        .format("%Y-%m-%dT%H:%M:%SZ")
                        .to_string()
                }),
                networks: Vec::new(), // Will be filled by caller
            })
        })
        .collect();

    Ok(items)
}
