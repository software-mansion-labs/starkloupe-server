use axum::{
    http::StatusCode,
    response::{IntoResponse, Response},
};
use simulate::SimulationArgs;
use starknet_api::block::BlockNumber;
use starknet_rust::providers::Provider;
use std::time::Duration;
use tokio::task;
use tokio::time::timeout;
use tracing::error;
use walnut_shared::create_rpc_client_from_url;

/// Default timeout for blocking simulation tasks (15 minutes).
const TASK_TIMEOUT_SECS: u64 = 900;

/// Resolve block number to a concrete value. If `args.block_number` is `None` (i.e. "Latest"),
/// queries the RPC endpoint for the current block number.
pub async fn resolve_block_number(args: &SimulationArgs) -> Result<Option<BlockNumber>, String> {
    if let Some(block_number) = args.block_number {
        return Ok(Some(block_number));
    }
    let provider_client = create_rpc_client_from_url(args.rpc_url.clone());
    provider_client
        .block_number()
        .await
        .map(|n| Some(BlockNumber(n)))
        .map_err(|e| format!("Failed to get latest block number: {}", e))
}

/// Wraps a closure in `spawn_blocking` + `timeout`. Returns the inner result on success,
/// or a standardized error `Response` for panics and timeouts.
pub async fn spawn_blocking_with_timeout<F, T>(f: F, context: &str) -> Result<T, Response>
where
    F: FnOnce() -> T + Send + 'static,
    T: Send + 'static,
{
    let simulation_task = task::spawn_blocking(f);
    match timeout(Duration::from_secs(TASK_TIMEOUT_SECS), simulation_task).await {
        Ok(Ok(result)) => Ok(result),
        Ok(Err(join_err)) => {
            error!("{} panicked: {:?}", context, join_err);
            Err((
                StatusCode::INTERNAL_SERVER_ERROR,
                "Failed to simulate transaction. Reach out to us for assistance.".to_string(),
            )
                .into_response())
        }
        Err(_) => {
            error!("{} timed out", context);
            Err((
                StatusCode::REQUEST_TIMEOUT,
                "The server timed out. Reach out to us for assistance.".to_string(),
            )
                .into_response())
        }
    }
}

/// Returns true if tracking (notifications) should be skipped based on the query parameter.
pub fn should_skip_tracking(skip_tracking: &Option<String>) -> bool {
    skip_tracking.as_deref().unwrap_or("") == "true"
}
