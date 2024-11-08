use crate::app_state::AppState;
use axum::{
    extract::{Path, Query, State},
    http::StatusCode,
    response::{IntoResponse, Response},
    Json,
};
use serde::{Deserialize, Serialize};
use starknet::core::types::Felt;
use starknet_api::core::ChainId;
use starknet_old::core::types as starknet_old_types;
use starknet_providers::Provider;
use std::str::FromStr;
use std::{collections::HashMap, sync::Arc};
use utoipa::ToSchema;
use verification::{db::fetch_verified_class, s3::fetch_verified_class_with_data};
use walnut_shared::{chain_id_to_readable_string, create_rpc_client, field_element_to_felt};
use walnut_shared::{extract_chain_id, felt_to_field_element};

#[derive(Deserialize, Debug, Serialize, ToSchema)]
pub struct ContractResponseWithSourceCode {
    pub chain_id: String,
    pub class_hash: String,
    pub is_class_verified: bool,
    pub source_code: Option<HashMap<String, String>>,
}

#[derive(Debug, Deserialize)]
pub struct QueryParams {
    include_source_code: Option<bool>,
}

#[utoipa::path(
    post,
    path = "/v1/{chain_id}/contracts/{contract_address}",
    responses(
        (status = 200, description = "Returns the contract data", body = ContractResponseWithSourceCode),
        (status = 404, description = "Contract not found for contract address", body = String)
    ),
    params(
        ("chain_id" = ChainId, Path, description = "Chain identifier"),
        ("contract_address" = String, Path, description = "Contract address"),
        ("include_source_code" = Option<bool>, Query, description = "Whether to include the source code in the response")
    ),
    tag = "Contract details"
)]
pub async fn get_contract_handler_with_chain_id(
    State(state): State<Arc<AppState>>,
    Path((chain_id, contract_address)): Path<(String, String)>,
    Query(query_params): Query<QueryParams>,
) -> Response {
    let chain_id = match extract_chain_id(chain_id.as_str()) {
        Ok(chain_id) => chain_id,
        Err(_) => return (StatusCode::BAD_REQUEST, "Invalid chain ID").into_response(),
    };

    let contracts = fetch_contract_data(&state, &contract_address, &query_params, &chain_id).await;

    if contracts.is_none() {
        (StatusCode::NOT_FOUND, "Contract not found").into_response()
    } else {
        (StatusCode::OK, Json(Some(contracts))).into_response()
    }
}

async fn fetch_contract_data(
    state: &Arc<AppState>,
    contract_address: &str,
    query_params: &QueryParams,
    chain_id: &ChainId,
) -> Option<ContractResponseWithSourceCode> {
    let provider_client = create_rpc_client(chain_id);
    let class_hash = provider_client
        .get_class_hash_at(
            starknet_old_types::BlockId::Tag(starknet_old_types::BlockTag::Latest),
            felt_to_field_element(Felt::from_str(contract_address).ok()?),
        )
        .await
        .ok()?;
    let class_hash_str = field_element_to_felt(class_hash).to_fixed_hex_string();

    if let Ok(_is_verified) = fetch_verified_class(&state.db_pool, &class_hash_str).await {
        let source_code = if query_params.include_source_code.unwrap_or(false) {
            match fetch_verified_class_with_data(&state.db_pool, &state.s3_client, &class_hash_str)
                .await
            {
                Ok((_, verified_class_data)) => Some(verified_class_data.source_code),
                Err(_) => None,
            }
        } else {
            None
        };

        Some(ContractResponseWithSourceCode {
            chain_id: chain_id_to_readable_string(chain_id),
            class_hash: class_hash_str,
            is_class_verified: true,
            source_code,
        })
    } else {
        Some(ContractResponseWithSourceCode {
            chain_id: chain_id_to_readable_string(chain_id),
            class_hash: class_hash_str,
            is_class_verified: false,
            source_code: None,
        })
    }
}
