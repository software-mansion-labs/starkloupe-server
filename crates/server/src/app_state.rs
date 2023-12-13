use sqlx::{Pool, Postgres};

pub struct AppState {
    pub db_pool: Pool<Postgres>,
    pub redis_client: redis::Client,
}
