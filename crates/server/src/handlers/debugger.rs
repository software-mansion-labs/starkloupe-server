use crate::app_state::AppState;
use crate::services::CacheKey;
use axum::{
    debug_handler,
    extract::State,
    http::StatusCode,
    response::{IntoResponse, Response},
    Json,
};
use simulate::DebugPayload;
use simulate::{debugger::debug_by_calldata, SimulationArgs};
use starknet_api::block::BlockNumber;
use starknet_rust::providers::Provider;
use std::sync::Arc;
use std::time::Duration;
use tokio::task;
use tokio::time::timeout;
use tracing::{error, info};
use walnut_shared::create_rpc_client_from_url;

#[debug_handler]
pub async fn debug_transaction(
    State(state): State<Arc<AppState>>,
    Json(payload): Json<DebugPayload>,
) -> Response {
    let db_pool = state.db_pool.clone();
    let s3_client = state.s3_client.clone();
    let cache = state.simulation_cache.clone();
    let external_cache = state.external_class_cache.clone();
    let voyager_client = state.voyager_client.clone();
    let payload = payload.clone();

    let simulation_task = task::spawn_blocking(move || {
        tokio::runtime::Handle::current().block_on(async move {
            // Parse debugger payload
            let debug_args = match SimulationArgs::try_from_debug_payload(payload).await {
                Ok(args) => args,
                Err(e) => {
                    return Err((StatusCode::BAD_REQUEST, e.to_string()));
                }
            };

            // Resolve block number if it's None (Latest)
            let resolved_block_number = if debug_args.block_number.is_none() {
                let provider_client = create_rpc_client_from_url(debug_args.rpc_url.clone());
                match provider_client.block_number().await {
                    Ok(block_number) => Some(BlockNumber(block_number)),
                    Err(e) => {
                        return Err((
                            StatusCode::BAD_REQUEST,
                            format!("Failed to get latest block number: {}", e),
                        ));
                    }
                }
            } else {
                debug_args.block_number.clone()
            };

            // Check debug cache first with resolved block number
            let cache_key = CacheKey::from_debug_args_with_block_number(
                &debug_args,
                resolved_block_number.as_ref(),
            );

            if let Some(cached_result) = cache.get_debug(&cache_key, Some(&db_pool)).await {
                info!(
                    "Debug cache hit - {}! Returning cached result",
                    cache_key.display_id()
                );
                return Ok((StatusCode::OK, cached_result)); // Return Arc directly - no clone!
            }
            info!(
                "Debug cache {} miss, proceeding with debug simulation",
                cache_key.display_id()
            );

            // Run debug simulation
            match debug_by_calldata(
                &db_pool,
                &s3_client,
                debug_args,
                Some(&external_cache),
                voyager_client.as_ref(),
            )
            .await
            {
                Ok(debug_info) => {
                    // Wrap in Arc and cache the debug result
                    let debug_info_arc = Arc::new(debug_info);
                    info!(
                        "Debug simulation completed for {}, caching result",
                        cache_key.display_id()
                    );
                    cache
                        .set_debug(&cache_key, debug_info_arc.clone(), Some(&db_pool))
                        .await;
                    Ok((StatusCode::OK, debug_info_arc))
                }
                Err(e) => {
                    info!("Debug simulation failed after: {}", e);
                    Err((StatusCode::BAD_REQUEST, e.to_string()))
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
