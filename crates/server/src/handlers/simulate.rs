use crate::app_state::AppState;
use axum::{
    debug_handler,
    extract::{Path, State},
    http::StatusCode,
    response::{IntoResponse, Response},
    Json,
};
use serde::{Deserialize, Serialize};
use simulate::{
    simulate::{simulate_by_calldata, simulate_transaction_by_hash},
    SimulationArgs, SimulationRawArgs,
};
use std::sync::Arc;
use axum::extract::Query;
use url::Url;
use walnut_shared::{extract_chain_id, rpc_url};
use crate::telegram_bot_service::send_telegram_notification;

#[derive(Serialize, Deserialize, Debug)]
pub enum SimulationPayload {
    WithCalldata(SimulationRawArgs),
    WithTxHash(SimulationTxHashArgs),
}

#[derive(Serialize, Deserialize, Debug)]
pub struct SimulationTxHashArgs {
    pub rpc_url: String,
    pub tx_hash: String,
}


#[derive(Debug, Deserialize)]
pub struct QueryParams {
    skip_tracking: Option<String>,
}

#[debug_handler]
pub async fn simulate_transaction(
    State(state): State<Arc<AppState>>,
    Json(payload): Json<SimulationPayload>,
) -> Response {
    let simulation_info = match payload {
        SimulationPayload::WithCalldata(args) => {
            let simulation_args: SimulationArgs = match SimulationArgs::try_from_raw_args(args).await {
                Ok(args) => args,
                Err(e) => return (StatusCode::BAD_REQUEST, Json(e.to_string())).into_response(),
            };

            simulate_by_calldata(&state.db_pool, &state.s3_client, simulation_args).await
        }
        SimulationPayload::WithTxHash(args) => {
            let rpc_url = match Url::parse(&args.rpc_url) {
                Ok(url) => url,
                Err(e) => return (StatusCode::BAD_REQUEST, Json(e.to_string())).into_response(),
            };

            simulate_transaction_by_hash(
                &state.db_pool,
                &state.s3_client,
                rpc_url,
                args.tx_hash,
                None,
            )
            .await
        }
    };

    match simulation_info {
        Ok(simulation_info) => (StatusCode::OK, Json(simulation_info)).into_response(),
        Err(e) => (StatusCode::BAD_REQUEST, Json(e.to_string())).into_response(),
    }
}

pub async fn simulate_transaction_by_hash_handler(
    State(state): State<Arc<AppState>>,
    Path((chain_id, tx_hash)): Path<(String, String)>,
    Query(query_params): Query<QueryParams>,
) -> Response {
    let chain_id = match extract_chain_id(chain_id.as_str()) {
        Ok(chain_id) => chain_id,
        Err(e) => return (StatusCode::BAD_REQUEST, Json(e.to_string())).into_response(),
    };

    // don't sent Telegram notification if query param skip_tg_notification=true (it set in URLs sent to tg bot)
    if !query_params.skip_tracking.unwrap_or(String::new()).eq("true") {
        send_telegram_notification(tx_hash.as_str(), &chain_id).await.unwrap();
    }

    let rpc_url = rpc_url(&chain_id);

    let simulation_info = simulate_transaction_by_hash(
        &state.db_pool,
        &state.s3_client,
        rpc_url,
        tx_hash,
        Some(chain_id),
    )
    .await;

    match simulation_info {
        Ok(simulation_info) => (StatusCode::OK, Json(simulation_info)).into_response(),
        Err(e) => (StatusCode::BAD_REQUEST, Json(e.to_string())).into_response(),
    }
}
