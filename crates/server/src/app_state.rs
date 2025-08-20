use sqlx::{Pool, Postgres};
use crate::services::SimulationCache;

pub struct AppState {
    pub db_pool: Pool<Postgres>,
    pub s3_client: aws_sdk_s3::Client,
    pub simulation_cache: SimulationCache,
}
