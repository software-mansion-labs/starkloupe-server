use axum::{
    extract::Path,
    http::StatusCode,
    response::{IntoResponse, Response},
    Json,
};
use serde::{Deserialize, Serialize};
use starknet::core::types::{BlockId, BlockTag, FieldElement};
use starknet::providers::Provider;
use starknet_api::core::ChainId;
use std::str::FromStr;
use utoipa::ToSchema;
use walnut_shared::{
    chain_id_to_readable_string, create_rpc_client, pad_field_element_to_hex_string_length66,
    MAIN_CHAIN_ID, SEPOLIA_CHAIN_ID,
};

#[derive(Deserialize, Debug, Serialize, ToSchema)]
pub struct Data {
    pub chain_id: String,
    pub hash: String,
}

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

#[utoipa::path(
    post,
    path = "/v1/search/{search_hash}",
    responses(
        (status = 200, description = "Returns the transaction, contract or class", body = SearchResponse),
        (status = 404, description = "Transaction, contract or class not found for the hash", body = String)
    ),
    params(
        ("search_hash" = String, Path, description = "Search for transaction, contract or class"),
    ),
    tag = "Search for transaction, contract or class"
)]
pub async fn get_search_handler(Path(search_hash): Path<String>) -> Response {
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

    match check_hash(&hash).await {
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

async fn check_hash(hash: &str) -> ESearchResult {
    let main_chain_id = ChainId(MAIN_CHAIN_ID.to_string());
    let sepolia_chain_id = ChainId(SEPOLIA_CHAIN_ID.to_string());

    if let Some(transactions) = check_transaction(hash, &[&main_chain_id, &sepolia_chain_id]).await
    {
        return ESearchResult::Transactions(transactions);
    }

    if let Some(contracts) = check_contract(hash, &[&main_chain_id, &sepolia_chain_id]).await {
        return ESearchResult::Contracts(contracts);
    }

    if let Some(classes) = check_class(hash, &[&main_chain_id, &sepolia_chain_id]).await {
        return ESearchResult::Classes(classes);
    }

    ESearchResult::None
}

async fn check_transaction(hash: &str, chain_ids: &[&ChainId]) -> Option<Vec<Data>> {
    let mut transactions = Vec::new();
    for chain_id in chain_ids {
        let provider_client = create_rpc_client(chain_id);
        match provider_client
            .get_transaction_status(FieldElement::from_str(hash).ok()?)
            .await
        {
            Ok(_) => {
                transactions.push(Data {
                    chain_id: chain_id_to_readable_string(chain_id),
                    hash: hash.to_string(),
                });
            }
            Err(_) => {
                continue;
            }
        }
    }
    if transactions.is_empty() {
        return None;
    }
    Some(transactions)
}

async fn check_contract(hash: &str, chain_ids: &[&ChainId]) -> Option<Vec<Data>> {
    let mut contracts = Vec::new();
    for chain_id in chain_ids {
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
                    chain_id: chain_id_to_readable_string(chain_id),
                    hash: hash.to_string(),
                });
            }
            Err(_) => {
                continue;
            }
        }
    }
    if contracts.is_empty() {
        return None;
    }
    Some(contracts)
}

async fn check_class(hash: &str, chain_ids: &[&ChainId]) -> Option<Vec<Data>> {
    let mut classes = Vec::new();
    for chain_id in chain_ids {
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
                    chain_id: chain_id_to_readable_string(chain_id),
                    hash: hash.to_string(),
                });
            }
            Err(_) => {
                continue;
            }
        }
    }
    if classes.is_empty() {
        return None;
    }
    Some(classes)
}
