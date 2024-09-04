use axum::{
    extract::{Path, Query},
    http::StatusCode,
    response::{IntoResponse, Response},
    Json,
};
use serde::{Deserialize, Serialize};
use starknet::core::types::{BlockId, BlockTag, FieldElement};
use starknet::providers::Provider;
use starknet_api::core::ChainId;
use std::str::FromStr;
use tracing::error;
use url::Url;
use utoipa::ToSchema;
use walnut_shared::{
    chain_id_to_readable_string, create_rpc_client, create_rpc_client_from_url,
    pad_field_element_to_hex_string_length66, MAIN_CHAIN_ID, SEPOLIA_CHAIN_ID,
};

#[derive(Deserialize, Debug, Serialize, ToSchema)]
pub enum Source {
    ChainId(String),
    RpcUrl(String),
}

#[derive(Deserialize, Debug, Serialize, ToSchema)]
pub struct Data {
    pub source: Source,
    pub hash: String,
}

#[derive(Deserialize, Debug, Serialize, ToSchema)]
pub struct SearchResponse {
    pub transactions: Vec<Data>,
    pub classes: Vec<Data>,
    pub contracts: Vec<Data>,
}

#[derive(Deserialize, Debug, Serialize, ToSchema)]
pub struct SearchQuery {
    pub rpc_urls: Option<String>,
}

#[derive(Debug)]
pub enum ESearchResult {
    Transactions(Vec<Data>),
    Contracts(Vec<Data>),
    Classes(Vec<Data>),
    None,
}

#[derive(Debug)]
pub enum SourceType {
    ChainId(ChainId),
    RpcUrl(Url),
}

#[utoipa::path(
    post,
    path = "/v1/search/{search_hash}",
    responses(
        (status = 200, description = "Returns the transaction, contract or class", body = SearchResponse),
        (status = 404, description = "Transaction, contract or class not found for the hash", body = String)
    ),
    params(
        ("search_hash" = String, Path, description = "Search for transaction, contract or class"),
        ("rpc_urls" = Option<String>, Query, description = "Comma-separated list of additional RPC URLs to check")
    ),
    tag = "Search for transaction, contract or class"
)]
pub async fn get_search_handler(
    Path(search_hash): Path<String>,
    Query(query): Query<SearchQuery>,
) -> Response {
    let hash = match FieldElement::from_str(search_hash.as_str()) {
        Ok(hash) => pad_field_element_to_hex_string_length66(hash),
        Err(err) => {
            return (
                StatusCode::BAD_REQUEST,
                format!("Search hash is invalid: {}", err),
            )
                .into_response()
        }
    };

    let mut sources = vec![
        SourceType::ChainId(ChainId(MAIN_CHAIN_ID.to_string())),
        SourceType::ChainId(ChainId(SEPOLIA_CHAIN_ID.to_string())),
    ];

    if let Some(urls) = query.rpc_urls {
        for url in urls.split(',') {
            let trimmed_url = url.trim();
            match Url::parse(trimmed_url) {
                Ok(parsed_url) => {
                    let host = parsed_url.host_str().unwrap_or("");
                    if host == "localhost" || host == "127.0.0.1" || host == "0.0.0.0" {
                        return (
                            StatusCode::BAD_REQUEST,
                            Json(format!(
                                "Invalid URL '{}': localhost or local IP addresses are not allowed",
                                trimmed_url
                            )),
                        )
                            .into_response();
                    }
                    sources.push(SourceType::RpcUrl(parsed_url));
                }
                Err(err) => {
                    return (
                        StatusCode::BAD_REQUEST,
                        Json(format!("Invalid URL '{}': {}", trimmed_url, err)),
                    )
                        .into_response();
                }
            }
        }
    }

    match check_hash(&hash, sources).await {
        ESearchResult::Transactions(transactions) => {
            let response = SearchResponse {
                transactions,
                classes: vec![],
                contracts: vec![],
            };
            (StatusCode::OK, Json(response)).into_response()
        }
        ESearchResult::Contracts(contracts) => {
            let response = SearchResponse {
                transactions: vec![],
                classes: vec![],
                contracts,
            };
            (StatusCode::OK, Json(response)).into_response()
        }
        ESearchResult::Classes(classes) => {
            let response = SearchResponse {
                transactions: vec![],
                classes,
                contracts: vec![],
            };
            (StatusCode::OK, Json(response)).into_response()
        }
        ESearchResult::None => {
            let response = SearchResponse {
                transactions: vec![],
                classes: vec![],
                contracts: vec![],
            };
            (StatusCode::NOT_FOUND, Json(response)).into_response()
        }
    }
}

async fn check_hash(hash: &str, sources: Vec<SourceType>) -> ESearchResult {
    if let Some(transactions) = check_transaction(hash, &sources).await {
        return ESearchResult::Transactions(transactions);
    }

    if let Some(contracts) = check_contract(hash, &sources).await {
        return ESearchResult::Contracts(contracts);
    }

    if let Some(classes) = check_class(hash, &sources).await {
        return ESearchResult::Classes(classes);
    }

    ESearchResult::None
}

async fn check_transaction(hash: &str, sources: &[SourceType]) -> Option<Vec<Data>> {
    let mut transactions = Vec::new();
    for source in sources {
        match source {
            SourceType::ChainId(chain_id) => {
                let provider_client = create_rpc_client(chain_id);
                match provider_client
                    .get_transaction_status(FieldElement::from_str(hash).ok()?)
                    .await
                {
                    Ok(_) => {
                        transactions.push(Data {
                            source: Source::ChainId(chain_id_to_readable_string(chain_id)),
                            hash: hash.to_string(),
                        });
                    }
                    Err(_) => continue,
                }
            }
            SourceType::RpcUrl(url) => {
                let client = create_rpc_client_from_url(url.clone());
                match client
                    .get_transaction_status(FieldElement::from_str(hash).ok()?)
                    .await
                {
                    Ok(_) => {
                        transactions.push(Data {
                            source: Source::RpcUrl(url.to_string()),
                            hash: hash.to_string(),
                        });
                    }
                    Err(_) => continue,
                }
            }
        }
    }

    if transactions.is_empty() {
        return None;
    }
    Some(transactions)
}

async fn check_contract(hash: &str, sources: &[SourceType]) -> Option<Vec<Data>> {
    let mut contracts = Vec::new();
    for source in sources {
        match source {
            SourceType::ChainId(chain_id) => {
                let provider_client = create_rpc_client(chain_id);
                match provider_client
                    .get_class_hash_at(
                        BlockId::Tag(BlockTag::Latest),
                        FieldElement::from_str(hash).ok()?,
                    )
                    .await
                {
                    Ok(_) => {
                        contracts.push(Data {
                            source: Source::ChainId(chain_id_to_readable_string(chain_id)),
                            hash: hash.to_string(),
                        });
                    }
                    Err(_) => continue,
                }
            }
            SourceType::RpcUrl(url) => {
                let client = create_rpc_client_from_url(url.clone());
                match client
                    .get_class_hash_at(
                        BlockId::Tag(BlockTag::Latest),
                        FieldElement::from_str(hash).ok()?,
                    )
                    .await
                {
                    Ok(_) => {
                        contracts.push(Data {
                            source: Source::RpcUrl(url.to_string()),
                            hash: hash.to_string(),
                        });
                    }
                    Err(_) => continue,
                }
            }
        }
    }

    if contracts.is_empty() {
        return None;
    }
    Some(contracts)
}

async fn check_class(hash: &str, sources: &[SourceType]) -> Option<Vec<Data>> {
    let mut classes = Vec::new();
    for source in sources {
        match source {
            SourceType::ChainId(chain_id) => {
                let provider_client = create_rpc_client(chain_id);
                match provider_client
                    .get_class(
                        BlockId::Tag(BlockTag::Latest),
                        FieldElement::from_str(hash).ok()?,
                    )
                    .await
                {
                    Ok(_) => {
                        classes.push(Data {
                            source: Source::ChainId(chain_id_to_readable_string(chain_id)),
                            hash: hash.to_string(),
                        });
                    }
                    Err(_) => continue,
                }
            }
            SourceType::RpcUrl(url) => {
                let client = create_rpc_client_from_url(url.clone());
                match client
                    .get_class(
                        BlockId::Tag(BlockTag::Latest),
                        FieldElement::from_str(hash).ok()?,
                    )
                    .await
                {
                    Ok(_) => {
                        classes.push(Data {
                            source: Source::RpcUrl(url.to_string()),
                            hash: hash.to_string(),
                        });
                    }
                    Err(_) => continue,
                }
            }
        }
    }

    if classes.is_empty() {
        return None;
    }
    Some(classes)
}
