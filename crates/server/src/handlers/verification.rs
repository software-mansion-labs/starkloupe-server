use crate::app_state::AppState;
use anyhow::Result;
use axum::{
    extract::{self, Path, State},
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    Json,
};
use serde::{Deserialize, Serialize};
use std::{collections::HashMap, sync::Arc};
use tracing::error;
use url::Url;
use utoipa::ToSchema;
use uuid::Uuid;
use verification::verification::verify_by_contract_address;
use verification::{
    db::fetch_verification_statuses_by_id, verification::initiate_verification,
    VerificationStatusSerializable,
};
use walnut_shared::{
    chain_id_to_readable_string, create_rpc_client, create_rpc_client_from_url, extract_chain_id,
    pad_hex_string_to_66,
};

#[derive(Deserialize, Debug, Serialize, ToSchema)]
pub struct VerificationStatusResponse {
    verification_statuses: Vec<VerificationStatusSerializable>,
    error_message: Option<String>,
}

#[utoipa::path(
    post,
    path = "/v1/verification/{verification_status_id}/status",
    responses(
        (status = 200, description = "Returns the status of the contract class verification", body = VerificationStatusResponse),
        (status = 404, description = "Verification status not found"),
        (status = 500, description = "Internal server error")
    ),
    params(
        ("verification_status_id" = String, Path, description = "Verification status identifier"),
    ),
    tag = "Contract class verification status"
)]
pub async fn get_verification_status_handler(
    State(state): State<Arc<AppState>>,
    Path(verification_status_id): Path<String>,
) -> Response {
    let verification_status_uuid = match Uuid::parse_str(&verification_status_id) {
        Ok(uuid) => uuid,
        Err(_) => {
            return (
                StatusCode::BAD_REQUEST,
                Json(VerificationStatusResponse {
                    verification_statuses: vec![],
                    error_message: Some(
                        "Invalid UUID format for verification_status_id.".to_string(),
                    ),
                }),
            )
                .into_response();
        }
    };

    match fetch_verification_statuses_by_id(&state.db_pool, verification_status_uuid).await {
        Ok(verification_status_rows) => (
            StatusCode::OK,
            Json(VerificationStatusResponse {
                verification_statuses: VerificationStatusSerializable::from_rows(
                    verification_status_rows,
                ),
                error_message: None,
            }),
        )
            .into_response(),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(VerificationStatusResponse {
                verification_statuses: vec![],
                error_message: Some(format!("Internal server error: {}", e)),
            }),
        )
            .into_response(),
    }
}

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
) -> Response {
    let chain_id = match extract_chain_id(chain_id.as_str()) {
        Ok(chain_id) => chain_id,
        Err(e) => {
            error!(
                chain_id = chain_id.as_str(),
                tags.verification_status = "failed",
                "Verification failed: {}",
                e.to_string(),
            );
            return (StatusCode::BAD_REQUEST, Json(e.to_string())).into_response();
        }
    };
    let chain_id_readable_string = chain_id_to_readable_string(&chain_id);
    let provider_client = create_rpc_client(&chain_id);
    let contract_address_clone = payload.contract_address.clone();

    match verify_by_contract_address(
        &state.db_pool,
        &state.s3_client,
        provider_client,
        payload.contract_address,
        payload.contract_name,
        payload.source_code,
        Some(chain_id_readable_string),
        None,
    )
    .await
    {
        Ok(verification_status_id) => {
            let response_message = format!(
                "Contract verification has started. You can check the verification status at the following link: https://app.walnut.dev/verification/status/{}",
                verification_status_id
            );
            (StatusCode::OK, Json(response_message)).into_response()
        }
        Err(e) => {
            error!(
                contract_address = contract_address_clone,
                tags.verification_status = "failed",
                "Verification failed: {}",
                e.to_string(),
            );
            (StatusCode::BAD_REQUEST, Json(e.to_string())).into_response()
        }
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
        "walnut_V7PlxSbPrpx_aalIqha6AqZwK0bB3juEzC" => Ok(9), // Unassigned
        "walnut_83emw3JcDMt_C6qXwh24Ni8ZmnMO5ni8c3" => Ok(10), // Unassigned
        "walnut_h2MmwIU99ru_2O4JkLWmNm9E6i9UXdpgFl" => Ok(11), // Unassigned
        "walnut_80tR2Eelg9Y_MzFziveCu37HjUBaUJGKr4" => Ok(12), // Unassigned
        "walnut_UAMEr3IpvRQ_tCjW5QYcf07mvE5T8mQgL6" => Ok(13), // Unassigned
        _ => Err(anyhow::anyhow!("Invalid API key")),
    }
}

#[derive(Deserialize, Debug, Serialize, ToSchema)]
pub struct VerificationPayloadWithRpc {
    pub class_name: Option<String>,
    pub class_hash: Option<String>,
    pub class_names: Option<Vec<String>>,
    pub class_hashes: Option<Vec<String>>,
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
        (status = 200, description = "Class verification has started", body = String),
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
) -> Response {
    let api_key = match get_api_token(&headers) {
        Ok(token) => token,
        Err(e) => {
            error!(
                tags.verification_status = "failed",
                "Verification failed: {}",
                e.to_string(),
            );
            return (StatusCode::BAD_REQUEST, Json(e.to_string())).into_response();
        }
    };

    let project_id = match check_api_key(&api_key) {
        Ok(project_id) => project_id,
        Err(e) => {
            error!(
                api_key = api_key,
                tags.verification_status = "failed",
                "Verification failed: {}",
                e.to_string(),
            );
            return (StatusCode::UNAUTHORIZED, Json(e.to_string())).into_response();
        }
    };

    let rpc_url = match Url::parse(&payload.rpc_url) {
        Ok(url) => url,
        Err(e) => {
            error!(
                project_id = project_id,
                tags.verification_status = "failed",
                "Verification failed: Failed to parse RPC URL: {}",
                e.to_string(),
            );
            return (StatusCode::BAD_REQUEST, Json(e.to_string())).into_response();
        }
    };

    let provider_client = create_rpc_client_from_url(rpc_url);

    let class_hashes = if let Some(hashes) = payload.class_hashes.clone() {
        hashes
    } else if let Some(hash) = payload.class_hash.clone() {
        vec![hash]
    } else {
        return (
            StatusCode::BAD_REQUEST,
            Json("Class hash is required".to_string()),
        )
            .into_response();
    };

    let class_hashes = class_hashes
        .iter()
        .map(|hash| pad_hex_string_to_66(hash))
        .collect();

    let class_names = if let Some(names) = payload.class_names.clone() {
        names
    } else if let Some(name) = payload.class_name.clone() {
        vec![name]
    } else {
        return (
            StatusCode::BAD_REQUEST,
            Json("Class name is required".to_string()),
        )
            .into_response();
    };

    let source_code = payload.source_code.clone();

    match initiate_verification(
        &state.db_pool,
        &state.s3_client,
        provider_client,
        class_hashes,
        class_names,
        source_code,
        None,
        Some(project_id),
    )
    .await
    {
        Ok(verification_status_id) => {
            let response_message = format!(
                "Contract verification has started. You can check the verification status at the following link: https://app.walnut.dev/verification/status/{}",
                verification_status_id
            );
            (StatusCode::OK, Json(response_message)).into_response()
        }
        Err(e) => {
            error!(
                project_id = project_id,
                tags.verification_status = "failed",
                "Verification failed: {}",
                e.to_string(),
            );
            (StatusCode::BAD_REQUEST, Json(e.to_string())).into_response()
        }
    }
}
