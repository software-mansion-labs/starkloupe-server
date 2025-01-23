use crate::{
    app_state::AppState,
    services::search::{check_class, sources_from_rpc_urls, ESource},
};
use axum::{
    extract::{Path, Query, State},
    http::StatusCode,
    response::{IntoResponse, Response},
    Json,
};
use serde::{Deserialize, Serialize};
use starknet::core::types::Felt;
use std::{collections::HashMap, sync::Arc};
use tracing::error;
use utoipa::ToSchema;
use verification::{
    db::fetch_verified_class,
    s3::{fetch_class_source_code, fetch_verified_class_with_data},
};

#[derive(Deserialize, Debug, Serialize, ToSchema)]
pub struct GetClassResponseWithSourceCode {
    pub source_code: HashMap<String, String>,
}

#[utoipa::path(
    post,
    path = "/v1/{chain_id}/classes/{class_hash}",
    responses(
        (status = 200, description = "Returns the verified contract class data", body = GetClassResponseWithSourceCode),
        (status = 404, description = "Contract class not found for the given chain_id and class_hash", body = String)
    ),
    params(
        ("chain_id" = ChainId, Path, description = "Chain identifier"),
        ("class_hash" = String, Path, description = "Contract class hash"),
    ),
    tag = "Contract class verification"
)]
pub async fn get_class_handler_with_chain_id(
    State(state): State<Arc<AppState>>,
    Path((_chain_id, class_hash)): Path<(String, String)>,
) -> Response {
    if let Ok((_verified_class_row, verified_class_data)) =
        fetch_verified_class_with_data(&state.db_pool, &state.s3_client, &class_hash).await
    {
        (
            StatusCode::OK,
            Json(GetClassResponseWithSourceCode {
                source_code: verified_class_data.source_code.clone(),
            }),
        )
            .into_response()
    } else {
        (
            StatusCode::NOT_FOUND,
            "Contract class is not verified or does not exist",
        )
            .into_response()
    }
}

#[derive(Deserialize, Debug, Serialize, ToSchema)]
pub struct GetClassResponse {
    pub verified: bool,
    pub declared_sources: Vec<ESource>,
    pub source_code: Option<HashMap<String, String>>,
}

#[derive(Deserialize, Debug, Serialize, ToSchema)]
pub struct ClassQuery {
    pub rpc_urls: Option<String>,
    pub include_source_code: Option<bool>,
}

#[utoipa::path(
    post,
    path = "/v1/classes/{class_hash}",
    responses(
        (status = 200, description = "Returns if the contract class is verified or not", body = GetClassResponse),
    ),
    params(
        ("class_hash" = String, Path, description = "Contract class hash"),
        ("include_source_code" = Option<bool>, Query, description = "Whether to include the source code in the response"),
        ("rpc_urls" = Option<String>, Query, description = "Comma-separated list of additional RPC URLs to check")

    ),
    tag = "Contract class details"
)]
pub async fn get_class_handler(
    State(state): State<Arc<AppState>>,
    Path(class_hash): Path<String>,
    Query(query): Query<ClassQuery>,
) -> Response {
    let class_hash_fixed = match Felt::from_hex(&class_hash) {
        Ok(felt) => felt.to_fixed_hex_string(),
        Err(_) => {
            let error_message = "Invalid class hash format";
            error!(error_message);
            return (StatusCode::BAD_REQUEST, Json(error_message)).into_response();
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

    let declared_sources = check_class(&class_hash_fixed, &sources)
        .await
        .unwrap_or_default()
        .into_iter()
        .map(|data| data.source)
        .collect();

    let is_verified = fetch_verified_class(&state.db_pool, &class_hash_fixed)
        .await
        .is_ok();

    let source_code = if query.include_source_code.unwrap_or(false) && is_verified {
        fetch_class_source_code(&state.s3_client, &class_hash_fixed)
            .await
            .ok()
    } else {
        None
    };

    (
        StatusCode::OK,
        Json(GetClassResponse {
            verified: is_verified,
            declared_sources,
            source_code,
        }),
    )
        .into_response()
}
