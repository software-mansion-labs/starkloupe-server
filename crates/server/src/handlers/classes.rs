use crate::app_state::AppState;
use axum::{
    extract::{Path, State},
    http::StatusCode,
    response::{IntoResponse, Response},
    Json,
};
use serde::{Deserialize, Serialize};
use std::{collections::HashMap, sync::Arc};
use utoipa::ToSchema;
use verification::{fetch_verified_class, fetch_verified_class_with_data};

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
        fetch_verified_class_with_data(&state.db_pool, &state.s3_client, class_hash).await
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
}

// TODO: Add includeSourceCode flag and return the source code if provided
#[utoipa::path(
    post,
    path = "/v1/classes/{class_hash}",
    responses(
        (status = 200, description = "Returns if the contract class is verified or not", body = GetClassResponse),
    ),
    params(
        ("class_hash" = String, Path, description = "Contract class hash"),
    ),
    tag = "Contract class verification"
)]
pub async fn get_class_handler(
    State(state): State<Arc<AppState>>,
    Path(class_hash): Path<String>,
) -> Response {
    if fetch_verified_class(&state.db_pool, class_hash)
        .await
        .is_ok()
    {
        (StatusCode::OK, Json(GetClassResponse { verified: true })).into_response()
    } else {
        (StatusCode::OK, Json(GetClassResponse { verified: false })).into_response()
    }
}
