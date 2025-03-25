use crate::app_state::AppState;
use crate::telegram_bot_service::{
    send_telegram_notification_calldata, send_telegram_notification_custom_rpc,
    send_telegram_notification_tx_id,
};
use axum::extract::Query;
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
use starknet_api::core::ChainId;
use std::sync::Arc;
use tracing::error;
use url::Url;
use walnut_shared::{extract_chain_id, get_rpc_urls, ENetwork};

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
    Query(query_params): Query<QueryParams>,
    Json(payload): Json<SimulationPayload>,
) -> Response {
    let simulation_info = match payload {
        SimulationPayload::WithCalldata(args) => {
            let simulation_args: SimulationArgs = match SimulationArgs::try_from_raw_args(args)
                .await
            {
                Ok(args) => args,
                Err(e) => return (StatusCode::BAD_REQUEST, Json(e.to_string())).into_response(),
            };
            if !query_params
                .skip_tracking
                .as_deref()
                .unwrap_or("")
                .eq("true")
            {
                if let Err(err) = send_telegram_notification_calldata(&simulation_args).await {
                    error!("Failed to send Telegram notification. Error: {:?}", err);
                }
            }
            simulate_by_calldata(&state.db_pool, &state.s3_client, simulation_args).await
        }
        SimulationPayload::WithTxHash(args) => {
            // don't sent Telegram notification if query param skip_tg_notification=true (it set in URLs sent to tg bot)
            if !query_params
                .skip_tracking
                .as_deref()
                .unwrap_or("")
                .eq("true")
            {
                if let Err(err) = send_telegram_notification_custom_rpc(
                    args.tx_hash.as_str(),
                    args.rpc_url.as_str(),
                )
                .await
                {
                    error!("Failed to send Telegram notification. Error: {:?}", err);
                }
            }
            let starknet_rpc_url = match Url::parse(&args.rpc_url) {
                Ok(url) => url,
                Err(e) => return (StatusCode::BAD_REQUEST, Json(e.to_string())).into_response(),
            };

            simulate_transaction_by_hash(
                &state.db_pool,
                &state.s3_client,
                Some(starknet_rpc_url),
                None,
                &args.tx_hash,
                None,
                &ENetwork::Starknet,
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
    // don't sent Telegram notification if query param skip_tg_notification=true (it set in URLs sent to tg bot)
    if !query_params
        .skip_tracking
        .as_deref()
        .unwrap_or("")
        .eq("true")
    {
        if let Err(err) =
            send_telegram_notification_tx_id(tx_hash.as_str(), chain_id.as_str()).await
        {
            error!("Failed to send Telegram notification. Error: {:?}", err);
        }
    }

    let (e_chain_id, network) = match extract_chain_id(chain_id.as_str()) {
        Ok((chain_id, network)) => (chain_id, network),
        Err(e) => return (StatusCode::BAD_REQUEST, Json(e.to_string())).into_response(),
    };

    let (starknet_rpc_url, etherem_rpc_url) = get_rpc_urls(&e_chain_id);

    let simulation_info = simulate_transaction_by_hash(
        &state.db_pool,
        &state.s3_client,
        starknet_rpc_url,
        etherem_rpc_url,
        &tx_hash,
        Some(e_chain_id),
        &network,
    )
    .await;

    match simulation_info {
        Ok(simulation_info) => (StatusCode::OK, Json(simulation_info)).into_response(),
        Err(e) => (StatusCode::BAD_REQUEST, Json(e.to_string())).into_response(),
    }
}
