use chrono::NaiveDateTime;
use sqlx::types::Uuid;

#[derive(Clone, Debug, Default)]
pub struct Simulation {
    pub id: Uuid,
    pub team_id: i32,
    pub chain_id: i32,
    pub block_at: i32,
    pub transaction_type: String,
    pub transaction_version: i32,
    // TODO: Tranasction fields starting with invoke
    pub created_at: NaiveDateTime,
    pub updated_at: NaiveDateTime,
}

#[derive(Clone, Debug)]
pub struct Team {
    pub id: i32,
}
