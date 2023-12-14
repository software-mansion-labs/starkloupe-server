use deadpool_redis;
use sqlx::{Pool, Postgres};

pub struct AppState {
    pub db_pool: Pool<Postgres>,
    pub redis_pool: deadpool_redis::Pool,
}
