use crate::app_state::AppState;
use crate::handlers::common::{resolve_block_number, spawn_blocking_with_timeout};
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
use std::sync::Arc;
use tracing::{error, info};

#[debug_handler]
pub async fn debug_transaction(
    State(state): State<Arc<AppState>>,
    Json(payload): Json<DebugPayload>,
) -> Response {
    let inner_result = spawn_blocking_with_timeout(
        move || {
            tokio::runtime::Handle::current().block_on(async move {
                let db_pool = &state.db_pool;

                // Parse debugger payload
                let debug_args = SimulationArgs::try_from_debug_payload(payload)
                    .await
                    .map_err(|e| (StatusCode::BAD_REQUEST, e.to_string()))?;

                // Resolve block number
                let resolved_block_number = resolve_block_number(&debug_args)
                    .await
                    .map_err(|e| (StatusCode::BAD_REQUEST, e))?;

                // Check debug cache
                let cache_key = CacheKey::from_debug_args_with_block_number(
                    &debug_args,
                    resolved_block_number.as_ref(),
                );
                if let Some(cached_result) = state
                    .simulation_cache
                    .get_debug(&cache_key, Some(db_pool))
                    .await
                {
                    info!(
                        "Debug cache hit {}! Returning cached result",
                        cache_key.display_id()
                    );
                    return Ok(cached_result);
                }
                info!(
                    "Debug cache miss {}, proceeding with debug simulation",
                    cache_key.display_id()
                );

                // Run debug simulation
                match debug_by_calldata(
                    db_pool,
                    &state.gcs_client,
                    debug_args,
                    Some(&state.external_class_cache),
                    state.voyager_client.as_ref(),
                    Some(&state.background_retry),
                )
                .await
                {
                    Ok(debug_info) => {
                        let debug_info_arc = Arc::new(debug_info);
                        info!(
                            "Debug simulation completed for {}, caching result",
                            cache_key.display_id()
                        );
                        state
                            .simulation_cache
                            .set_debug(&cache_key, debug_info_arc.clone(), Some(db_pool))
                            .await;
                        Ok(debug_info_arc)
                    }
                    Err(e) => {
                        error!("Debug simulation failed: {}", e);
                        Err((StatusCode::BAD_REQUEST, e.to_string()))
                    }
                }
            })
        },
        "Debug simulation task",
    )
    .await;

    match inner_result {
        Ok(Ok(debug_info)) => (StatusCode::OK, Json(debug_info)).into_response(),
        Ok(Err((status, message))) => (status, Json(message)).into_response(),
        Err(timeout_or_panic) => timeout_or_panic,
    }
}
