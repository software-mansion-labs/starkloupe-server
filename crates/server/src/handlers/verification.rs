use crate::app_state::AppState;
use anyhow::Result;
use axum::{
    extract::{self, State},
    http::{HeaderMap, StatusCode},
    Json,
};
use serde::{Deserialize, Serialize};
use std::{collections::HashMap, sync::Arc};
use utoipa::ToSchema;
use verification::{verify_by_class_hash, verify_by_contract_address};
use walnut_shared::{
    chain_id_to_readable_string, create_rpc_client, create_rpc_client_from_url, extract_chain_id,
};

#[derive(Deserialize, Debug, Serialize, ToSchema)]
pub struct VerificationPayload {
    pub contract_name: String,
    pub contract_address: String,
    #[schema(
        example = "{ \"src/lib.cairo\": \"// lib.cairo source code\", \"src/utils/util1.cairo\": \"// util1.cairo source code\" }"
    )]
    pub source_code: HashMap<String, String>,
}

#[utoipa::path(
    post,
    path = "/v1/{chain_id}/verify",
    request_body(
        content = VerificationPayload,
        description = "Contract name, address, and source code to verify",
        content_type = "application/json"
    ),
    responses(
        (status = 200, description = "Contract successfully verified", body = String),
        (status = 400, description = "An error occurred during verification; an error message will be returned", body = String)
    ),
    params(
        ("chain_id" = ChainId, Path, description = "Chain identifier"),
    ),
    tag = "Contract class verification"
)]
pub async fn verify_handler(
    State(state): State<Arc<AppState>>,
    chain_id: extract::Path<String>,
    Json(payload): Json<VerificationPayload>,
) -> (StatusCode, String) {
    let chain_id = extract_chain_id(chain_id.as_str());
    let chain_id_readable_string = chain_id_to_readable_string(chain_id.clone());
    let provider_client = create_rpc_client(&chain_id);
    match verify_by_contract_address(
        &state.db_pool,
        &state.s3_client,
        provider_client,
        payload.contract_address,
        payload.contract_name,
        payload.source_code,
        Some(chain_id),
        None,
    )
    .await
    {
        Ok(class_hash) => (StatusCode::OK, format!("Contract has been successfully verified. You can check the verification status at the following link: https://api.walnut.dev/v1/{chain_id_readable_string}/classes/{class_hash}.").to_string()),
        Err(e) => (StatusCode::BAD_REQUEST, e.to_string()),
    }
}

fn get_api_token(headers: &HeaderMap) -> Result<String> {
    match headers.get("x-api-key") {
        Some(value) => Ok(value.to_str()?.to_string()),
        None => Err(anyhow::anyhow!("Missing x-api-key header")),
    }
}

fn check_api_key(api_key: &str) -> Result<i32> {
    match api_key {
        "walnut_ZFqJep8VrMB_LfUXdSeKxJAxNz9AC6rdLK" => Ok(1), // Walnut Project
        "walnut_cntgR78e35j_SjkgMzV0KrNykHY9F0pVjB" => Ok(8), // Cartridge Project
        "walnut_V7PlxSbPrpx_aalIqha6AqZwK0bB3juEzC" => Ok(9), // SomeProject Project
        _ => Err(anyhow::anyhow!("Invalid API key")),
    }
}

#[derive(Deserialize, Debug, Serialize, ToSchema)]
pub struct VerificationPayloadWithRpc {
    pub class_name: String,
    pub class_hash: String,
    pub rpc_url: String,
    #[schema(
        example = "{ \"src/lib.cairo\": \"// lib.cairo source code\", \"src/utils/util1.cairo\": \"// util1.cairo source code\" }"
    )]
    pub source_code: HashMap<String, String>,
}

#[utoipa::path(
    post,
    path = "/v1/verify",
    request_body(
        content = VerificationPayloadWithRpc,
        description = "Class name, class hash, RPC URL, and source code to verify",
        content_type = "application/json"
    ),
    responses(
        (status = 200, description = "Class successfully verified", body = String),
        (status = 400, description = "An error occurred during verification; an error message will be returned", body = String)
    ),
    params(
        ("x-api-key" = String, Header, description = "Walnut API key"),
    ),
    tag = "Contract class verification"
)]

pub async fn verify_handler_with_rpc(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Json(payload): Json<VerificationPayloadWithRpc>,
) -> (StatusCode, String) {
    let api_key = match get_api_token(&headers) {
        Ok(token) => token,
        Err(e) => return (StatusCode::BAD_REQUEST, e.to_string()),
    };

    let project_id = match check_api_key(&api_key) {
        Ok(project_id) => project_id,
        Err(e) => return (StatusCode::UNAUTHORIZED, e.to_string()),
    };

    let provider_client = match create_rpc_client_from_url(&payload.rpc_url) {
        Ok(client) => client,
        Err(e) => return (StatusCode::BAD_REQUEST, e.to_string()),
    };

    match verify_by_class_hash(
        &state.db_pool,
        &state.s3_client,
        provider_client,
        payload.class_hash.clone(),
        payload.class_name,
        payload.source_code,
        None,
        Some(project_id)
    ).await
    {
        Ok(()) => (StatusCode::OK, format!("Class has been successfully verified. You can check the verification status at the following link: https://api.walnut.dev/v1/classes/{}.", payload.class_hash).to_string()),
        Err(e) => (StatusCode::BAD_REQUEST, e.to_string()),
    }
}
