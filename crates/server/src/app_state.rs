use sqlx::{Pool, Postgres};

pub struct AppState {
    pub db_pool: Pool<Postgres>,
    pub s3_client: aws_sdk_s3::Client,
}
