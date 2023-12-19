use chrono::NaiveDateTime;
use serde::{Deserialize, Serialize};
use sqlx::types::Uuid;

#[derive(Clone, Debug, Default)]
pub struct Simulation {
    pub id: Uuid,
    pub project_id: i32,
    pub chain_id: String,
    pub block_at: i32,
    pub transaction_version: i32,
    pub nonce: i32,
    pub max_fee: String,
    pub cairo_version: String,
    pub wallet_address: String,
    pub calldata: Vec<String>,
    pub created_at: NaiveDateTime,
    pub updated_at: NaiveDateTime,
    pub status: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Project {
    pub id: i32,
    pub name: String,
}

#[derive(Clone, Debug)]
pub struct User {
    pub email: String,
}
