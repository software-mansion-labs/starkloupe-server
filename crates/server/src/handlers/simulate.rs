use crate::app_state::AppState;
use crate::calldata_encoder;
use crate::handlers::common::{
    resolve_block_number, should_skip_tracking, spawn_blocking_with_timeout,
};
use crate::notification_service::{
    send_notification_calldata, send_notification_custom_rpc, send_notification_tx_id,
};
use crate::services::CacheKey;
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
use tracing::{debug, error, info, instrument};
use walnut_shared::{chain_id_to_readable_string, extract_chain_id, get_rpc_urls, ENetwork};

#[derive(Serialize, Deserialize, Debug, Clone)]
// Variant names are part of the JSON wire format; keep the `With` prefix.
#[allow(clippy::enum_variant_names)]
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

#[derive(Serialize, Debug)]
pub struct SimulationErrorResponse {
    pub error: String,
    pub simulation_args: SimulationPayload,
}

/// Build a BAD_REQUEST error response with the original payload for debugging.
fn sim_error_response(error: String, simulation_args: SimulationPayload) -> Response {
    (
        StatusCode::BAD_REQUEST,
        Json(SimulationErrorResponse {
            error,
            simulation_args,
        }),
    )
        .into_response()
}

// ---------------------------------------------------------------------------
// Shared simulation logic
// ---------------------------------------------------------------------------

/// Build a short human-readable label from simulation args for log messages.
fn sim_log_label(args: &SimulationArgs) -> String {
    let sender = args.sender_address.0.key().to_fixed_hex_string();
    let sender_short = if sender.len() > 10 {
        format!("{}..{}", &sender[..6], &sender[sender.len() - 4..])
    } else {
        sender
    };
    let block = args
        .block_number
        .map(|b| b.0.to_string())
        .unwrap_or_else(|| "latest".to_string());
    let chain = chain_id_to_readable_string(&args.chain_id);
    format!("sender={} block={} chain={}", sender_short, block, chain)
}

/// Resolve block number, check cache, send notification, run simulation, cache result.
/// Shared by both `WithCalldata` and `WithDecodedCalldata` paths.
#[instrument(name = "run_sim_with_cache", skip_all, fields(
    sender = %simulation_args.sender_address.0.key().to_fixed_hex_string(),
    chain_id = %simulation_args.chain_id,
    block_number = ?simulation_args.block_number.map(|b| b.0),
))]
async fn run_simulation_with_cache(
    state: &AppState,
    simulation_args: SimulationArgs,
    skip_tracking: bool,
    payload_for_error: SimulationPayload,
) -> Result<Arc<simulate::TransactionSimulationResult>, Response> {
    let db_pool = &state.db_pool;
    let label = sim_log_label(&simulation_args);

    // Resolve block number
    let resolved_block_number = resolve_block_number(&simulation_args)
        .await
        .map_err(|e| sim_error_response(e, payload_for_error.clone()))?;

    // Check cache
    let cache_key = CacheKey::from_simulation_args_with_block_number(
        &simulation_args,
        resolved_block_number.as_ref(),
    );
    if let Some(cached_result) = state.simulation_cache.get(&cache_key, Some(db_pool)).await {
        info!(
            "Cache hit {} ({}), returning cached result",
            cache_key.display_id(),
            label
        );
        return Ok(cached_result);
    }
    info!(
        "Cache miss {} ({}), proceeding with simulation",
        cache_key.display_id(),
        label
    );

    // Notification
    if !skip_tracking {
        if let Err(err) = send_notification_calldata(&simulation_args).await {
            error!("Failed to send Telegram notification. Error: {:?}", err);
        }
    }

    // Run simulation
    match simulate_by_calldata(
        db_pool,
        &state.s3_client,
        simulation_args,
        state.voyager_client.as_ref(),
        Some(&state.external_class_cache),
        Some(&state.background_retry),
    )
    .await
    {
        Ok(sim_info) => {
            let sim_info_arc = Arc::new(sim_info);
            state
                .simulation_cache
                .set(&cache_key, sim_info_arc.clone(), Some(db_pool))
                .await;
            info!(
                "Cached simulation result {} ({})",
                cache_key.display_id(),
                label
            );
            Ok(sim_info_arc)
        }
        Err(e) => {
            error!("Simulation failed ({}): {}", label, e);
            Err(sim_error_response(e.to_string(), payload_for_error))
        }
    }
}

// ---------------------------------------------------------------------------
// Per-variant handlers (called inside spawn_blocking / block_on)
// ---------------------------------------------------------------------------

#[instrument(name = "handle_calldata_sim", skip_all, fields(sender = %args.sender_address))]
async fn handle_calldata_simulation(
    state: &AppState,
    args: SimulationRawArgs,
    skip_tracking: bool,
) -> Result<Arc<simulate::TransactionSimulationResult>, Response> {
    let args_for_error = args.clone();

    let simulation_args = SimulationArgs::try_from_raw_args(args).await.map_err(|e| {
        sim_error_response(
            e.to_string(),
            SimulationPayload::WithCalldata(args_for_error.clone()),
        )
    })?;

    run_simulation_with_cache(
        state,
        simulation_args,
        skip_tracking,
        SimulationPayload::WithCalldata(args_for_error),
    )
    .await
}

#[instrument(name = "handle_decoded_sim", skip_all, fields(sender = %args.sender_address))]
async fn handle_decoded_calldata_simulation(
    state: &AppState,
    args: SimulationDecodedArgs,
    skip_tracking: bool,
) -> Result<Arc<simulate::TransactionSimulationResult>, Response> {
    let decoded_args_for_error = args.clone();

    let chain_id = args.chain_id.as_deref().ok_or_else(|| {
        sim_error_response(
            "Missing required field: chain_id".to_string(),
            SimulationPayload::WithDecodedCalldata(decoded_args_for_error.clone()),
        )
    })?;

    let raw_calldata = calldata_encoder::encode_decoded_calldata(&args.decoded_calldata, chain_id)
        .await
        .map_err(|e| {
            sim_error_response(
                format!("Failed to encode decoded calldata: {}", e),
                SimulationPayload::WithDecodedCalldata(decoded_args_for_error),
            )
        })?;

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

    // From here on, behaves identically to WithCalldata
    handle_calldata_simulation(state, raw_args, skip_tracking).await
}

#[instrument(name = "handle_tx_hash_sim", skip_all, fields(tx_hash = %args.tx_hash))]
async fn handle_tx_hash_simulation(
    state: &AppState,
    args: SimulationTxHashArgs,
    skip_tracking: bool,
) -> Result<Arc<simulate::TransactionSimulationResult>, Response> {
    let db_pool = &state.db_pool;
    let args_for_error = args.clone();

    // Check cache
    let cache_key = CacheKey::from_tx_hash(&args.tx_hash, "starknet");
    if let Some(cached_result) = state.simulation_cache.get(&cache_key, Some(db_pool)).await {
        debug!("Cache hit for tx hash! Returning cached result");
        return Ok(cached_result);
    }

    // Notification
    if !skip_tracking {
        if let Err(err) =
            send_notification_custom_rpc(args.tx_hash.as_str(), args.rpc_url.as_str()).await
        {
            error!("Failed to send Telegram notification. Error: {:?}", err);
        }
    }

    let starknet_rpc_url = url::Url::parse(&args.rpc_url).map_err(|e| {
        sim_error_response(
            format!("Invalid RPC URL: {}", e),
            SimulationPayload::WithTxHash(args_for_error.clone()),
        )
    })?;

    match simulate_transaction_by_hash(
        db_pool,
        &state.s3_client,
        Some(starknet_rpc_url),
        None,
        &args.tx_hash,
        None,
        &ENetwork::Starknet,
        state.voyager_client.as_ref(),
        Some(&state.external_class_cache),
        Some(&state.background_retry),
    )
    .await
    {
        Ok(sim_info) => {
            let sim_info_arc = Arc::new(sim_info);
            state
                .simulation_cache
                .set(&cache_key, sim_info_arc.clone(), Some(db_pool))
                .await;
            info!("Cached tx hash simulation result");
            Ok(sim_info_arc)
        }
        Err(e) => {
            error!("Tx hash simulation failed: {}", e);
            Err(sim_error_response(
                e.to_string(),
                SimulationPayload::WithTxHash(args_for_error),
            ))
        }
    }
}

// ---------------------------------------------------------------------------
// Public handler endpoints
// ---------------------------------------------------------------------------

#[debug_handler]
#[instrument(name = "simulate_transaction", skip_all)]
pub async fn simulate_transaction(
    State(state): State<Arc<AppState>>,
    Query(query_params): Query<QueryParams>,
    Json(payload): Json<SimulationPayload>,
) -> Response {
    let skip_tracking = should_skip_tracking(&query_params.skip_tracking);

    let inner_result = spawn_blocking_with_timeout(
        move || {
            tokio::runtime::Handle::current().block_on(async move {
                match payload {
                    SimulationPayload::WithCalldata(args) => {
                        handle_calldata_simulation(&state, args, skip_tracking).await
                    }
                    SimulationPayload::WithDecodedCalldata(args) => {
                        handle_decoded_calldata_simulation(&state, args, skip_tracking).await
                    }
                    SimulationPayload::WithTxHash(args) => {
                        handle_tx_hash_simulation(&state, args, skip_tracking).await
                    }
                }
            })
        },
        "Simulation task",
    )
    .await;

    match inner_result {
        Ok(Ok(sim_info)) => (StatusCode::OK, Json(sim_info)).into_response(),
        Ok(Err(error_response)) => error_response,
        Err(timeout_or_panic) => timeout_or_panic,
    }
}

#[instrument(name = "simulate_by_hash_handler", skip(state), fields(chain_id = %chain_id, tx_hash = %tx_hash))]
pub async fn simulate_transaction_by_hash_handler(
    State(state): State<Arc<AppState>>,
    Path((chain_id, tx_hash)): Path<(String, String)>,
    Query(query_params): Query<QueryParams>,
) -> Response {
    // Notification is sent before the blocking task (matches original behavior)
    if !should_skip_tracking(&query_params.skip_tracking) {
        if let Err(err) = send_notification_tx_id(tx_hash.as_str(), chain_id.as_str()).await {
            error!("Failed to send Telegram notification. Error: {:?}", err);
        }
    }

    let (e_chain_id, network) = match extract_chain_id(chain_id.as_str()) {
        Ok((chain_id, network)) => (chain_id, network),
        Err(e) => return (StatusCode::BAD_REQUEST, Json(e.to_string())).into_response(),
    };

    let (starknet_rpc_url, ethereum_rpc_url) = get_rpc_urls(&e_chain_id);
    let e_chain_id = Some(e_chain_id);

    let chain_id_for_cache = chain_id.clone();
    let inner_result = spawn_blocking_with_timeout(
        move || {
            tokio::runtime::Handle::current().block_on(async move {
                let db_pool = &state.db_pool;

                // Check cache
                let cache_key = CacheKey::from_tx_hash(&tx_hash, &chain_id_for_cache);
                if let Some(cached_result) =
                    state.simulation_cache.get(&cache_key, Some(db_pool)).await
                {
                    info!(
                        "Cache hit for tx hash={} handler! Returning cached result",
                        tx_hash
                    );
                    return Ok(cached_result);
                }
                info!("Cache miss for tx_hash={}, starting simulation", tx_hash);

                match simulate_transaction_by_hash(
                    db_pool,
                    &state.s3_client,
                    starknet_rpc_url,
                    ethereum_rpc_url,
                    &tx_hash,
                    e_chain_id,
                    &network,
                    state.voyager_client.as_ref(),
                    Some(&state.external_class_cache),
                    Some(&state.background_retry),
                )
                .await
                {
                    Ok(sim_info) => {
                        let sim_info_arc = Arc::new(sim_info);
                        state
                            .simulation_cache
                            .set(&cache_key, sim_info_arc.clone(), Some(db_pool))
                            .await;
                        info!("Cached simulation by tx hash: {}", tx_hash);
                        Ok(sim_info_arc)
                    }
                    Err(e) => {
                        error!("Simulation by tx hash: {} failed: {}", tx_hash, e);
                        Err(e)
                    }
                }
            })
        },
        "Simulation by tx hash",
    )
    .await;

    match inner_result {
        Ok(Ok(simulation_info)) => (StatusCode::OK, Json(simulation_info)).into_response(),
        Ok(Err(e)) => (StatusCode::BAD_REQUEST, Json(e.to_string())).into_response(),
        Err(timeout_or_panic) => timeout_or_panic,
    }
}
