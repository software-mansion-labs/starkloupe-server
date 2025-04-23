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
use std::sync::Arc;
use std::time::Duration;
use tokio::task;
use tokio::time::timeout;
use tracing::error;
use walnut_shared::{extract_chain_id, get_rpc_urls, ENetwork};

#[derive(Serialize, Deserialize, Debug, Clone)]
pub enum SimulationPayload {
    WithCalldata(SimulationRawArgs),
    WithTxHash(SimulationTxHashArgs),
}

#[derive(Serialize, Deserialize, Debug, Clone)]
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
    let db_pool = state.db_pool.clone();
    let s3_client = state.s3_client.clone();
    let skip_tracking = query_params.skip_tracking.clone();
    let payload = payload.clone();

    let simulation_task = task::spawn_blocking(move || {
        tokio::runtime::Handle::current().block_on(async move {
            match payload {
                SimulationPayload::WithCalldata(args) => {
                    // Parse calldata args
                    let simulation_args: SimulationArgs =
                        match SimulationArgs::try_from_raw_args(args).await {
                            Ok(args) => args,
                            Err(e) => {
                                return Err((StatusCode::BAD_REQUEST, e.to_string()));
                            }
                        };

                    // Telegram notification
                    if !skip_tracking.as_deref().unwrap_or("").eq("true") {
                        if let Err(err) =
                            send_telegram_notification_calldata(&simulation_args).await
                        {
                            error!("Failed to send Telegram notification. Error: {:?}", err);
                        }
                    }

                    // Run simulation
                    match simulate_by_calldata(&db_pool, &s3_client, simulation_args).await {
                        Ok(sim_info) => Ok((StatusCode::OK, sim_info)),
                        Err(e) => Err((StatusCode::BAD_REQUEST, e.to_string())),
                    }
                }

                SimulationPayload::WithTxHash(args) => {
                    // Telegram notification
                    if !skip_tracking.as_deref().unwrap_or("").eq("true") {
                        if let Err(err) = send_telegram_notification_custom_rpc(
                            args.tx_hash.as_str(),
                            args.rpc_url.as_str(),
                        )
                        .await
                        {
                            error!("Failed to send Telegram notification. Error: {:?}", err);
                        }
                    }

                    let starknet_rpc_url = match url::Url::parse(&args.rpc_url) {
                        Ok(url) => url,
                        Err(e) => return Err((StatusCode::BAD_REQUEST, e.to_string())),
                    };

                    match simulate_transaction_by_hash(
                        &db_pool,
                        &s3_client,
                        Some(starknet_rpc_url),
                        None,
                        &args.tx_hash,
                        None,
                        &ENetwork::Starknet,
                    )
                    .await
                    {
                        Ok(sim_info) => Ok((StatusCode::OK, sim_info)),
                        Err(e) => Err((StatusCode::BAD_REQUEST, e.to_string())),
                    }
                }
            }
        })
    });

    match timeout(Duration::from_secs(600), simulation_task).await {
        Ok(Ok(Ok((status, sim_info)))) => (status, Json(sim_info)).into_response(),
        Ok(Ok(Err((status, message)))) => (status, Json(message)).into_response(),
        Ok(Err(join_err)) => {
            error!("Simulation task panicked: {:?}", join_err);
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                "Failed to simulation transaction. Reach out to us for assistance.".to_string(),
            )
                .into_response()
        }
        Err(_) => {
            error!("Simulation transaction request timed out");
            (
                StatusCode::REQUEST_TIMEOUT,
                "The server timed out. Reach out to us for assistance.".to_string(),
            )
                .into_response()
        }
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

    let db_pool = state.db_pool.clone();
    let s3_client = state.s3_client.clone();
    let tx_hash = tx_hash.clone();
    let payload_tx_hash = tx_hash.clone();
    let network = network.clone();
    let e_chain_id = Some(e_chain_id);

    let simulation_task = task::spawn_blocking(move || {
        tokio::runtime::Handle::current().block_on(async move {
            simulate_transaction_by_hash(
                &db_pool,
                &s3_client,
                starknet_rpc_url,
                etherem_rpc_url,
                &tx_hash,
                e_chain_id,
                &network,
            )
            .await
        })
    });

    // Wait for simulation with timeout
    match timeout(Duration::from_secs(600), simulation_task).await {
        Ok(Ok(Ok(simulation_info))) => (StatusCode::OK, Json(simulation_info)).into_response(),
        Ok(Ok(Err(e))) => (StatusCode::BAD_REQUEST, Json(e.to_string())).into_response(),
        Ok(Err(join_err)) => {
            error!("Simulation of tx panicked: {:?}", join_err);
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                "Failed to simulation transaction. Reach out to us for assistance.".to_string(),
            )
                .into_response()
        }
        Err(_) => {
            error!(
                "Simulation transaction request by tx hash  timed out {}: {}",
                &chain_id, &payload_tx_hash
            );
            (
                StatusCode::REQUEST_TIMEOUT,
                "The server timed out. Reach out to us for assistance.".to_string(),
            )
                .into_response()
        }
    }
}
