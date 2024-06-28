use crate::app_state::AppState;
use axum::{
    extract::{self, State},
    http::StatusCode,
    Json,
};
use serde::{Deserialize, Serialize};
use std::{collections::HashMap, sync::Arc};
use utoipa::ToSchema;
use verification::verify_by_contract_address;
use walnut_shared::extract_chain_id;

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
    match verify_by_contract_address(
        &state.db_pool,
        &state.s3_client,
        chain_id,
        payload.contract_address,
        payload.contract_name,
        payload.source_code,
    )
    .await
    {
        Ok(_) => (StatusCode::OK, "Contract verified".to_string()),
        Err(e) => (StatusCode::BAD_REQUEST, e.to_string()),
    }
}
