use chrono::NaiveDateTime;
use sqlx::types::Uuid;

#[derive(Debug, Default)]
pub struct Simulation {
    pub id: Uuid,
    pub team_id: i32,
    pub chain_id: i32,
    pub block_at: i32,
    pub transaction_type: String,
    pub transaction_version: i32,
    pub created_at: NaiveDateTime,
    pub updated_at: NaiveDateTime,
}
