use crate::services::SimulationCache;
use internal_tracing::background_retry::BackgroundRetryService;
use internal_tracing::external_class_cache::ExternalClassCache;
use sqlx::{Pool, Postgres};
use verification::voyager::VoyagerClient;

pub struct AppState {
    pub db_pool: Pool<Postgres>,
    pub gcs_client: google_cloud_storage::client::Storage,
    pub simulation_cache: SimulationCache,
    pub external_class_cache: ExternalClassCache,
    pub voyager_client: Option<VoyagerClient>,
    pub background_retry: BackgroundRetryService,
}
