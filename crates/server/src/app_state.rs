use crate::services::SimulationCache;
use internal_tracing::external_class_cache::ExternalClassCache;
use sqlx::{Pool, Postgres};
use verification::voyager::VoyagerClient;

pub struct AppState {
    pub db_pool: Pool<Postgres>,
    pub s3_client: aws_sdk_s3::Client,
    pub simulation_cache: SimulationCache,
    pub external_class_cache: ExternalClassCache,
    pub voyager_client: Option<VoyagerClient>,
}
