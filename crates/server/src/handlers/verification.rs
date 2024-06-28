use crate::app_state::AppState;
use axum::{
    extract::{self, State},
    http::StatusCode,
    Json,
};
use serde::Deserialize;
use std::{collections::HashMap, sync::Arc};
use verification::verify_by_contract_address;
use walnut_shared::extract_chain_id;

#[derive(Deserialize, Debug)]
pub struct VerificationPayload {
    pub contract_name: String,
    pub contract_address: String,
    pub source_code: HashMap<String, String>,
}

pub async fn verify_handler(
    State(state): State<Arc<AppState>>,
    chain_id: extract::Path<String>,
    Json(payload): Json<VerificationPayload>,
) -> (StatusCode, String) {
    // state.db_pool.
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
