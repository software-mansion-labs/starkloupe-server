use crate::services::search::{
    check_class, check_contract, check_transaction, sources_from_rpc_urls, Data, ESourceType,
};
use axum::{
    extract::{Path, Query},
    http::StatusCode,
    response::{IntoResponse, Response},
    Json,
};
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;
use walnut_shared::felt_str_to_fixed;

#[derive(Deserialize, Debug, Serialize, ToSchema)]
pub struct SearchResponse {
    pub transactions: Vec<Data>,
    pub classes: Vec<Data>,
    pub contracts: Vec<Data>,
}

#[derive(Debug)]
pub enum ESearchResult {
    Transactions(Vec<Data>),
    Contracts(Vec<Data>),
    Classes(Vec<Data>),
    None,
}

#[derive(Deserialize, Debug, Serialize, ToSchema)]
pub struct SearchQuery {
    pub rpc_urls: Option<String>,
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
    let hash = match felt_str_to_fixed(search_hash.as_str()) {
        Ok(hash) => hash,
        Err(err) => {
            return (
                StatusCode::BAD_REQUEST,
                format!("Search hash is invalid: {}", err),
            )
                .into_response()
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

async fn check_hash(hash: &str, sources: Vec<ESourceType>) -> ESearchResult {
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
