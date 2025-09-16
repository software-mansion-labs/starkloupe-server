use crate::app_state::AppState;
use crate::calldata_encoder;
use crate::services::CacheKey;
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
use data_decoder::DecodedValue;
use serde::{Deserialize, Serialize};
use simulate::{
    simulate::{simulate_by_calldata, simulate_transaction_by_hash},
    SimulationArgs, SimulationRawArgs,
};
use std::sync::Arc;
use std::time::Duration;
use tokio::task;
use tokio::time::timeout;
use tracing::{error, info};
use walnut_shared::{extract_chain_id, get_rpc_urls, ENetwork, create_rpc_client_from_url};
use starknet_api::block::BlockNumber;
use starknet::providers::Provider;

#[derive(Serialize, Deserialize, Debug, Clone)]
pub enum SimulationPayload {
    WithCalldata(SimulationRawArgs),
    WithDecodedCalldata(SimulationDecodedArgs),
    WithTxHash(SimulationTxHashArgs),
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct SimulationDecodedArgs {
    pub chain_id: Option<String>,
    pub rpc_url: Option<String>,
    pub block_number: Option<u64>,
    pub nonce: Option<u64>,
    pub sender_address: String,
    pub decoded_calldata: Vec<ContractCall>,
    pub transaction_version: usize,
    pub transaction_signature: Option<Vec<String>>,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct ContractCall {
    pub contract_address: String,
    pub function_selector: String,
    pub function_name: Option<String>,
    pub parameters: Vec<DecodedValue>,
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

                    // Resolve block number if it's None (Latest)
                    let resolved_block_number = if simulation_args.block_number.is_none() {
                        let provider_client = create_rpc_client_from_url(simulation_args.rpc_url.clone());
                        match provider_client.block_number().await {
                            Ok(block_number) => Some(BlockNumber(block_number)),
                            Err(e) => {
                                return Err((StatusCode::BAD_REQUEST, format!("Failed to get latest block number: {}", e)));
                            }
                        }
                    } else {
                        simulation_args.block_number.clone()
                    };

                    // Check cache first with resolved block number
                    let cache_key = CacheKey::from_simulation_args_with_block_number(&simulation_args, resolved_block_number.as_ref());
                    if let Some(cached_result) = state.simulation_cache.get(&cache_key).await {
                        info!("Cache hit! Returning cached result");
                        return Ok((StatusCode::OK, cached_result)); // Return Arc directly - no clone!
                    }
                    info!("Cache miss, proceeding with simulation");

                    // Telegram notification
                    if !skip_tracking.as_deref().unwrap_or("").eq("true") {
                        if let Err(err) =
                            send_telegram_notification_calldata(&simulation_args).await
                        {
                            error!("Failed to send Telegram notification. Error: {:?}", err);
                        }
                    }

                    // Run simulation
                    let result = simulate_by_calldata(&db_pool, &s3_client, simulation_args).await;
                    match result {
                        Ok(sim_info) => {
                            // Wrap in Arc and cache the result
                            let sim_info_arc = Arc::new(sim_info);
                            state
                                .simulation_cache
                                .set(&cache_key, sim_info_arc.clone())
                                .await;
                            info!("Cached simulation result");

                            Ok((StatusCode::OK, sim_info_arc))
                        }
                        Err(e) => {
                            info!("Simulation failed after: {}", e);
                            Err((StatusCode::BAD_REQUEST, e.to_string()))
                        }
                    }
                }

                SimulationPayload::WithDecodedCalldata(args) => {
                    // Convert decoded calldata to raw calldata with ABI information
                    let chain_id = args.chain_id.as_deref().ok_or_else(|| {
                        (
                            StatusCode::BAD_REQUEST,
                            "Missing required field: chain_id".to_string(),
                        )
                    })?;
                    let raw_calldata = match calldata_encoder::encode_decoded_calldata(
                        &args.decoded_calldata,
                        chain_id,
                    )
                    .await
                    {
                        Ok(calldata) => calldata,
                        Err(e) => {
                            return Err((
                                StatusCode::BAD_REQUEST,
                                format!("Failed to encode decoded calldata: {}", e),
                            ));
                        }
                    };

                    let raw_args = SimulationRawArgs {
                        chain_id: args.chain_id,
                        rpc_url: args.rpc_url,
                        block_number: args.block_number,
                        nonce: args.nonce,
                        sender_address: args.sender_address,
                        calldata: raw_calldata,
                        transaction_version: args.transaction_version,
                        transaction_signature: args
                            .transaction_signature
                            .map(|sig| sig.into_iter().filter_map(|s| s.parse().ok()).collect()),
                    };

                    // Parse calldata args
                    let simulation_args: SimulationArgs =
                        match SimulationArgs::try_from_raw_args(raw_args).await {
                            Ok(args) => args,
                            Err(e) => {
                                return Err((StatusCode::BAD_REQUEST, e.to_string()));
                            }
                        };

                    // Check cache first
                    let cache_key = CacheKey::from_simulation_args(&simulation_args);
                    if let Some(cached_result) = state.simulation_cache.get(&cache_key).await {
                        info!("Cache hit! Returning cached result");
                        return Ok((StatusCode::OK, cached_result));
                    }
                    info!("Cache miss, proceeding with simulation");

                    // Telegram notification
                    if !skip_tracking.as_deref().unwrap_or("").eq("true") {
                        if let Err(err) =
                            send_telegram_notification_calldata(&simulation_args).await
                        {
                            error!("Failed to send Telegram notification. Error: {:?}", err);
                        }
                    }

                    // Run simulation
                    let result = simulate_by_calldata(&db_pool, &s3_client, simulation_args).await;
                    match result {
                        Ok(sim_info) => {
                            // Wrap in Arc and cache the result
                            let sim_info_arc = Arc::new(sim_info);
                            state
                                .simulation_cache
                                .set(&cache_key, sim_info_arc.clone())
                                .await;
                            info!("Cached simulation result");

                            Ok((StatusCode::OK, sim_info_arc))
                        }
                        Err(e) => {
                            info!("Simulation failed after: {}", e);
                            Err((StatusCode::BAD_REQUEST, e.to_string()))
                        }
                    }
                }

                SimulationPayload::WithTxHash(args) => {
                    // Check cache first using tx hash
                    let cache_key = CacheKey::from_tx_hash(&args.tx_hash, "starknet");

                    if let Some(cached_result) = state.simulation_cache.get(&cache_key).await {
                        info!("Cache hit for tx hash! Returning cached result");
                        return Ok((StatusCode::OK, cached_result));
                    }
                    info!("Cache miss for tx hash, proceeding with simulation");
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
                        Ok(sim_info) => {
                            // Wrap in Arc and cache the result
                            let sim_info_arc = Arc::new(sim_info);
                            state
                                .simulation_cache
                                .set(&cache_key, sim_info_arc.clone())
                                .await;
                            info!("Cached tx hash simulation result");

                            Ok((StatusCode::OK, sim_info_arc))
                        }
                        Err(e) => {
                            info!("Tx hash simulation failed after: {}", e);
                            Err((StatusCode::BAD_REQUEST, e.to_string()))
                        }
                    }
                }
            }
        })
    });

    match timeout(Duration::from_secs(900), simulation_task).await {
        Ok(Ok(Ok((status, sim_info)))) => (status, Json(sim_info)).into_response(),
        Ok(Ok(Err((status, message)))) => (status, Json(message)).into_response(),
        Ok(Err(join_err)) => {
            error!("Simulation task panicked: {:?}", join_err);
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                "Failed to simulate transaction. Reach out to us for assistance.".to_string(),
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

    let cache = state.simulation_cache.clone();
    let chain_id_clone = chain_id.clone();
    let simulation_task = task::spawn_blocking(move || {
        tokio::runtime::Handle::current().block_on(async move {
            // Check cache first
            let cache_key = CacheKey::from_tx_hash(&tx_hash, &chain_id_clone);

            if let Some(cached_result) = cache.get(&cache_key).await {
                info!("Cache hit for tx hash handler! Returning cached result");
                return Ok(cached_result);
            }
            info!("Cache miss for tx hash handler, proceeding with simulation");

            let result = simulate_transaction_by_hash(
                &db_pool,
                &s3_client,
                starknet_rpc_url,
                etherem_rpc_url,
                &tx_hash,
                e_chain_id,
                &network,
            )
            .await;

            match result {
                Ok(sim_info) => {
                    // Wrap in Arc and cache the result
                    let sim_info_arc = Arc::new(sim_info);
                    cache.set(&cache_key, sim_info_arc.clone()).await;
                    info!("Cached simulation by hash result");

                    Ok(sim_info_arc)
                }
                Err(e) => {
                    info!("Simulation by hash failed after: {}", e);
                    Err(e)
                }
            }
        })
    });

    // Wait for simulation with timeout
    match timeout(Duration::from_secs(900), simulation_task).await {
        Ok(Ok(Ok(simulation_info))) => (StatusCode::OK, Json(simulation_info)).into_response(),
        Ok(Ok(Err(e))) => (StatusCode::BAD_REQUEST, Json(e.to_string())).into_response(),
        Ok(Err(join_err)) => {
            error!("Simulation of tx panicked: {:?}", join_err);
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                "Failed to simulate transaction. Reach out to us for assistance.".to_string(),
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

// Commented out cache stats endpoint - using logging instead
// pub async fn cache_stats_handler(State(state): State<Arc<AppState>>) -> Response {
//     match serde_json::to_value(state.simulation_cache.stats().await) {
//         Ok(stats) => (StatusCode::OK, Json(stats)).into_response(),
//         Err(e) => (
//             StatusCode::INTERNAL_SERVER_ERROR,
//             Json(format!("Failed to get cache stats: {}", e)),
//         )
//             .into_response(),
//     }
// }
