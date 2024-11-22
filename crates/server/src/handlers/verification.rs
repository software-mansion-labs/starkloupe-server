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
use verification::minimal_verification::initiate_minimal_verification;
use verification::verification::{verify_by_class_hash, verify_by_contract_address};
use verification::{
    db::fetch_verification_statuses_by_id, manifest::Manifest, verification::initiate_verification,
    VerificationRequestRow, VerificationStatusSerializable,
};
use walnut_shared::{
    chain_id_to_readable_string, create_rpc_client, create_rpc_client_from_url, extract_chain_id,
    felt_str_to_fixed, parse_version_string_to_tuple,
};

#[derive(Deserialize, Debug, Serialize, ToSchema)]
pub struct VerificationStatusResponse {
    verification_request: Option<VerificationRequestRow>,
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
                    verification_request: None,
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
        Ok((verification_request, verification_status_rows)) => (
            StatusCode::OK,
            Json(VerificationStatusResponse {
                verification_request,
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
                verification_request: None,
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
    pub contract_address: Option<String>,
    pub class_hash: Option<String>,
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
        description = "Contract name, contract address or class hash, and source code to verify",
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
    if let Some(class_hash) = payload.class_hash {
        let class_hash_fixed = match felt_str_to_fixed(&class_hash) {
            Ok(fixed) => fixed,
            Err(e) => {
                let error_message = format!("Failed to convert class hash: {}", e.to_string());
                error!(error_message);
                return (StatusCode::BAD_REQUEST, Json(error_message)).into_response();
            }
        };
        let class_hash_fixed_clone = class_hash_fixed.clone();
        match verify_by_class_hash(
            &state.db_pool,
            &state.s3_client,
            provider_client,
            class_hash_fixed,
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
                    class_hash = class_hash_fixed_clone,
                    tags.verification_status = "failed",
                    "Verification failed: {}",
                    e.to_string(),
                );
                (StatusCode::BAD_REQUEST, Json(e.to_string())).into_response()
            }
        }
    } else if let Some(contract_address) = payload.contract_address {
        let contract_address_clone = contract_address.clone();
        match verify_by_contract_address(
            &state.db_pool,
            &state.s3_client,
            provider_client,
            contract_address,
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
    } else {
        (
            StatusCode::BAD_REQUEST,
            Json("The required parameter is missing - please provide a valid class hash or contract address".to_string()),
        )
            .into_response()
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
        "walnut_V7PlxSbPrpx_aalIqha6AqZwK0bB3juEzC" => Ok(9),
        "walnut_83emw3JcDMt_C6qXwh24Ni8ZmnMO5ni8c3" => Ok(10),
        "walnut_h2MmwIU99ru_2O4JkLWmNm9E6i9UXdpgFl" => Ok(11),
        "walnut_80tR2Eelg9Y_MzFziveCu37HjUBaUJGKr4" => Ok(12),
        "walnut_UAMEr3IpvRQ_tCjW5QYcf07mvE5T8mQgL6" => Ok(13),

        "walnut_tW5YiM75v9a_MRmjmAu1qGiRZ2dDNN323o" => Ok(14),
        "walnut_SlmxTnqPEST_C41RY5wE5ycbhU4XoJjlcm" => Ok(15),
        "walnut_XIuhAujgaaF_PtjWmx3kUlZsJWTmJ2dTAw" => Ok(16),
        "walnut_S6kKZutIQlQ_jR79512PTly2JOukqhf785" => Ok(17),
        "walnut_RQ3pC4cqOkk_fIyCmODlj8NK6L1E7rPEP3" => Ok(18),
        "walnut_StoZLiQGqw2_HozDhQQ8CcPOsqu2POzObD" => Ok(19),
        "walnut_bz6AXYZUe1o_jk9s4CzvK1PfYvSDEOSmcu" => Ok(20),
        "walnut_ED6LtVdJdxX_U4HfCT4WIc6GCi3E8AMZGq" => Ok(21),
        "walnut_LGXu5j6klpv_01I25NQEKBRanv0ntJbz5d" => Ok(22),
        "walnut_593mlD3yTTF_yquD5HzXrdb1h5DmdSwmoy" => Ok(23),
        _ => Err(anyhow::anyhow!("Invalid API key")),
    }
}

#[derive(Deserialize, Debug, Serialize, ToSchema)]
pub struct VerificationPayloadWithRpc {
    pub class_name: Option<String>,
    pub class_hash: Option<String>,
    pub class_names: Option<Vec<String>>,
    pub class_hashes: Option<Vec<String>>,
    pub rpc_url: Option<String>,
    #[schema(
        example = "{ \"src/lib.cairo\": \"// lib.cairo source code\", \"src/utils/util1.cairo\": \"// util1.cairo source code\" }"
    )]
    pub source_code: HashMap<String, String>,
    pub cairo_version: Option<String>,
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
    Json(mut payload): Json<VerificationPayloadWithRpc>,
) -> Response {
    let api_key = get_api_token(&headers).ok();

    let project_id = match api_key {
        Some(ref key) => match check_api_key(key) {
            Ok(project_id) => Some(project_id),
            Err(e) => {
                error!(
                    api_key = api_key,
                    tags.verification_status = "failed",
                    "Verification failed: {}",
                    e.to_string(),
                );
                return (StatusCode::UNAUTHORIZED, Json(e.to_string())).into_response();
            }
        },
        None => None,
    };

    if let Some(rpc_url) = &payload.rpc_url {
        let rpc_url = match Url::parse(rpc_url) {
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

        let class_hashes = match class_hashes
            .iter()
            .map(|hash| felt_str_to_fixed(hash))
            .collect::<Result<Vec<_>, _>>()
        {
            Ok(hashes) => hashes,
            Err(e) => {
                return (
                    StatusCode::BAD_REQUEST,
                    Json(format!("Failed to convert class hash: {}", e.to_string())),
                )
                    .into_response();
            }
        };

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

        match initiate_verification(
            &state.db_pool,
            &state.s3_client,
            provider_client,
            class_hashes,
            class_names,
            payload.source_code,
            None,
            project_id,
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
    } else {
        let cairo_version = if let Some(version_str) = payload.cairo_version.as_deref() {
            match parse_version_string_to_tuple(version_str) {
                Ok(version) => Some(version),
                Err(e) => {
                    let error_message = format!("Failed to parse Cairo version: {}", e.to_string());
                    error!(tags.verification_status = "failed", error_message);
                    return (StatusCode::BAD_REQUEST, Json(error_message)).into_response();
                }
            }
        } else {
            None
        };

        let manifest = match Manifest::new(&mut payload.source_code, cairo_version) {
            Ok(manifest) => manifest,
            Err(e) => {
                error!(
                    tags.verification_status = "failed",
                    "Failed to create manifest: {}",
                    e.to_string(),
                );
                return (StatusCode::BAD_REQUEST, Json(e.to_string())).into_response();
            }
        };

        match initiate_minimal_verification(
            &state.db_pool,
            &state.s3_client,
            payload.source_code,
            manifest,
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
}
