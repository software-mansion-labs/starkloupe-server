use crate::services::SimulationCache;
use internal_tracing::background_retry::BackgroundRetryService;
use internal_tracing::external_class_cache::ExternalClassCache;
use moka::future::Cache;
use sqlx::{Pool, Postgres};
use uuid::Uuid;
use verification::voyager::VoyagerClient;

pub struct AppState {
    pub db_pool: Pool<Postgres>,
    pub s3_client: aws_sdk_s3::Client,
    pub simulation_cache: SimulationCache,
    pub external_class_cache: ExternalClassCache,
    pub voyager_client: Option<VoyagerClient>,
    pub background_retry: BackgroundRetryService,
    pub api_key_cache: Cache<[u8; 32], Uuid>,
}
