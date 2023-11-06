use crate::app_state::AppState;
use crate::db;
use axum::Json;
use serde::{Deserialize, Serialize};
use std::sync::Arc;

#[derive(Serialize, Deserialize)]
struct StarkNetTransaction {
    // Define your transaction structure here;
}

pub async fn simulate(Json(payload): Json<StarkNetTransaction>, state: Arc<AppState>) {
    // TODO: Get current block number from node

    // TODO: Insert into database
    let sim = db::Simulation::default();
    // Insert into database
    sqlx::query!(
        "INSERT INTO simulations (team_id, chain_id, block_at, transaction_type, transaction_version) VALUES ($1, $2, $3, $4, $5)",
        sim.team_id,
        sim.chain_id,
        sim.block_at,
        sim.transaction_type,
        sim.transaction_version,
    ).execute(&state.db_pool).await.unwrap();

    // TODO(jainkunal): Execute the transaction in context of the block and get status

    // TODO(jainkunal): Update status to DB

    // TODO: Return
}
