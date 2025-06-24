use crate::app_state::AppState;
use axum::{
    debug_handler,
    extract::{ State},
    http::StatusCode,
    response::{IntoResponse, Response},
    Json,
};
use simulate::DebugPayload;
use simulate::{debugger::debug_by_calldata, SimulationArgs};
use std::sync::Arc;
use std::time::Duration;
use tokio::task;
use tokio::time::timeout;
use tracing::error;

#[debug_handler]
pub async fn debug_transaction(
    State(state): State<Arc<AppState>>,
    Json(payload): Json<DebugPayload>,
) -> Response {
    let db_pool = state.db_pool.clone();
    let s3_client = state.s3_client.clone();
    let payload = payload.clone();

    let simulation_task = task::spawn_blocking(move || {
        tokio::runtime::Handle::current().block_on(async move {
            // Parse debugger payload
            let debug_args = match SimulationArgs::try_from_debug_payload(payload).await {
                Ok(args) => args,
                Err(e) => return Err((StatusCode::BAD_REQUEST, e.to_string())),
            };

            match debug_by_calldata(&db_pool, &s3_client, debug_args).await {
                Ok(sim_info) => Ok((StatusCode::OK, sim_info)),
                Err(e) => Err((StatusCode::BAD_REQUEST, e.to_string())),
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
