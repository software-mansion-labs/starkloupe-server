use anyhow::Context;
use axum::{
    extract::Path,
    http::StatusCode,
    response::{IntoResponse, Response},
    Json,
};
use serde::{Deserialize, Serialize};
use starknet::core::types::ContractClass;
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
pub struct GetContractResponse {
    pub chain_id: String,
    pub class_hash: String,
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
pub async fn get_contract_handler(Path(contract_address): Path<String>) -> Response {
    let main_chain_id = ChainId(MAIN_CHAIN_ID.to_string());
    let sepolia_chain_id = ChainId(SEPOLIA_CHAIN_ID.to_string());

    let mut contracts: Vec<GetContractResponse> = Vec::new();
    get_contract_data(&mut contracts, contract_address.as_str(), &main_chain_id).await;
    get_contract_data(&mut contracts, contract_address.as_str(), &sepolia_chain_id).await;

    if contracts.is_empty() {
        (StatusCode::NOT_FOUND, "Contract not found").into_response()
    } else {
        (StatusCode::OK, Json(contracts)).into_response()
    }
}

async fn get_contract_data(
    contracts: &mut Vec<GetContractResponse>,
    contract_address: &str,
    chain_id: &ChainId,
) {
    let provider_client = create_rpc_client(chain_id);
    let class_hash = provider_client
        .get_class_hash_at(
            BlockId::Tag(BlockTag::Latest),
            FieldElement::from_str(contract_address)
                .context("Contract address format is incorrect")
                .unwrap(),
        )
        .await
        .context(format!("Can't find the contract class on {}", chain_id));

    if let Ok(class_hash) = class_hash {
        contracts.push(GetContractResponse {
            chain_id: chain_id_to_readable_string(chain_id),
            class_hash: pad_field_element_to_hex_string_length66(class_hash),
        });
    }
}
