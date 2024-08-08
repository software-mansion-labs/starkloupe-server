use crate::app_state::AppState;
use anyhow::Context;
use axum::{
    extract::{Path, State},
    http::StatusCode,
    response::{IntoResponse, Response},
    Json,
};
use serde::{Deserialize, Serialize};
use starknet::core::types::{BlockId, BlockTag, FieldElement};
use starknet::providers::Provider;
use starknet_api::core::ChainId;
use std::str::FromStr;
use std::{collections::HashMap, sync::Arc};
use utoipa::ToSchema;
use verification::{fetch_verified_class, fetch_verified_class_with_data};
use walnut_shared::{
    chain_id_to_readable_string, create_rpc_client, pad_field_element_to_hex_string_length66,
    MAIN_CHAIN_ID, SEPOLIA_CHAIN_ID,
};

#[derive(Deserialize, Debug, Serialize, ToSchema)]
pub struct GetContractResponseWithSourceCode {
    pub chain_id: String,
    pub class_hash: String,
    pub source_code: Option<HashMap<String, String>>,
}

#[utoipa::path(
    post,
    path = "/v1/contracts/{contract_address}",
    responses(
        (status = 200, description = "Returns the contract data", body = GetContractResponseWithSourceCode),
        (status = 404, description = "Contract not found for contract address", body = String)
    ),
    params(
        ("contract_address" = String, Path, description = "Contract address"),
    ),
    tag = "Contract details"
)]
pub async fn get_contract_handler(
    State(state): State<Arc<AppState>>,
    Path(contract_address): Path<String>,
) -> Response {
    let main_chain_id = ChainId(MAIN_CHAIN_ID.to_string());
    let sepolia_chain_id = ChainId(SEPOLIA_CHAIN_ID.to_string());

    let contracts = fetch_contracts(
        &state,
        &contract_address,
        &[&main_chain_id, &sepolia_chain_id],
    )
    .await;

    if contracts.is_empty() {
        (StatusCode::NOT_FOUND, "Contract not found").into_response()
    } else {
        (StatusCode::OK, Json(contracts)).into_response()
    }
}

async fn fetch_contracts(
    state: &Arc<AppState>,
    contract_address: &str,
    chain_ids: &[&ChainId],
) -> Vec<GetContractResponseWithSourceCode> {
    let mut contracts = Vec::new();
    for &chain_id in chain_ids {
        if let Some(contract_data) = fetch_contract_data(state, contract_address, chain_id).await {
            contracts.push(contract_data);
        }
    }
    contracts
}

async fn fetch_contract_data(
    state: &Arc<AppState>,
    contract_address: &str,
    chain_id: &ChainId,
) -> Option<GetContractResponseWithSourceCode> {
    let provider_client = create_rpc_client(chain_id);
    let class_hash = provider_client
        .get_class_hash_at(
            BlockId::Tag(BlockTag::Latest),
            FieldElement::from_str(contract_address).ok()?,
        )
        .await
        .ok()?;
    let class_hash_str = pad_field_element_to_hex_string_length66(class_hash);

    let contract_data =
        fetch_verified_class_with_data(&state.db_pool, &state.s3_client, class_hash_str.clone())
            .await;

    Some(match contract_data {
        Ok((_verified_class_row, verified_class_data)) => GetContractResponseWithSourceCode {
            chain_id: chain_id_to_readable_string(chain_id),
            class_hash: class_hash_str,
            source_code: Some(verified_class_data.source_code),
        },
        Err(_) => GetContractResponseWithSourceCode {
            chain_id: chain_id_to_readable_string(chain_id),
            class_hash: class_hash_str,
            source_code: None,
        },
    })
}
