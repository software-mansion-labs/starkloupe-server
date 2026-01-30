extern crate dotenv;
mod abi_fetcher;
mod app_state;
mod appsmith_api;
mod binaries_manager_service;
mod calldata_encoder;
mod handlers;
mod services;
mod telegram_bot_service;

use app_state::AppState;
use aws_sdk_s3::config::Region;
use aws_sdk_s3::Client;
use axum::{routing::get, routing::post, Router};
use axum_prometheus::PrometheusMetricLayer;
use dotenv::dotenv;
use handlers::{
    calldata_decoder::decode_calldata_handler,
    classes::{get_class_handler, get_contracts_by_class_hash_handler},
    contracts::{get_contract_entrypoints_handler, get_contract_handler},
    debugger::debug_transaction,
    openapi::ApiDoc,
    simulate::{simulate_transaction, simulate_transaction_by_hash_handler},
    verification::verify_handler,
};
use sqlx::postgres::PgPoolOptions;
use std::sync::Arc;
use tower_http::cors::CorsLayer;
use tracing_subscriber::{fmt, layer::SubscriberExt, util::SubscriberInitExt, EnvFilter};
use utoipa::OpenApi;

use crate::appsmith_api::get_verification_data;
use crate::binaries_manager_service::{
    download_scarb_and_sozo_binaries_from_s3, start_github_dojo_binaries_downloader_scheduler,
    start_github_scarb_binaries_downloader_scheduler,
};
use crate::handlers::{
    classes::get_class_handler_with_chain_id,
    search::get_search_handler,
    verification::{get_verification_status_handler, verify_handler_with_rpc},
};
use crate::services::SimulationCache;
use axum::extract::State;
use axum::http::StatusCode;
use tokio::time::{timeout, Duration};
// Resources
// https://github.com/tokio-rs/axum/tree/main/examples
// https://www.apianalytics.dev/
// - https://github.com/tom-draper/api-analytics
// https://docs.rs/axum-prometheus/latest/axum_prometheus/

fn main() -> Result<(), Box<dyn std::error::Error>> {
    dotenv().ok();
    // SENTRY CONFIGURATION
    // _guard must be defined on top level so Sentry will catch errors
    // Also Tokio must be initialized manually (not with attribute)
    let mut _guard;
    // Start Sentry only in RELEASE build
    if !cfg!(debug_assertions) {
        _guard = sentry::init(("https://ae2d01aafee9ea77f4090092df5a6a42@o4507958254436352.ingest.us.sentry.io/4507961681838080", sentry::ClientOptions {
            release: sentry::release_name!(),
            ..sentry::ClientOptions::default()
        }));
    }

    tracing_subscriber::registry()
        .with(fmt::layer())
        .with(sentry_tracing::layer()) // Logging to stdout
        .with(EnvFilter::new(
            std::env::var("LOG_LEVEL").unwrap_or("INFO".to_string()),
        )) // Set the maximum log level to INFO
        .init();

    tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .unwrap()
        .block_on(async {
            let db_addr = std::env::var("DATABASE_URL").unwrap_or("postgres://".to_string());
            let db_pool = PgPoolOptions::new()
                .max_connections(30)
                .connect(&db_addr)
                .await?;

            // Configure the region and endpoint
            let region = Region::new(std::env::var("S3_REGION").unwrap_or("".to_string()));
            let shared_config = aws_config::from_env()
                .region(region)
                .endpoint_url(std::env::var("S3_ENDPOINT").unwrap_or("".to_string()))
                .load()
                .await;

            // Create the S3 client
            let s3_client = Client::new(&shared_config);
            sqlx::migrate!().run(&db_pool).await?;

            // Download scarb and sozo binaries
            download_scarb_and_sozo_binaries_from_s3(&s3_client).await?;
            start_github_scarb_binaries_downloader_scheduler().await;
            start_github_dojo_binaries_downloader_scheduler().await;

            // Initialize simulation cache with configurable settings from environment
            let cache_capacity = std::env::var("CACHE_CAPACITY")
                .unwrap_or_else(|_| "100".to_string()) // Production: 100 entries
                .parse::<u64>()
                .unwrap_or(100);
            let cache_ttl_minutes = std::env::var("CACHE_TTL_MINUTES")
                .unwrap_or_else(|_| "1440".to_string()) // Production: 24 hours (24 * 60 = 1440 minutes)
                .parse::<u64>()
                .unwrap_or(1440);

            let simulation_cache = SimulationCache::new(cache_capacity, cache_ttl_minutes);

            // Initialize external class cache for Voyager compiled classes
            let external_class_cache =
                internal_tracing::external_class_cache::ExternalClassCache::from_env();

            // Initialize Voyager client with hardcoded API key
            let voyager_config = verification::voyager::VoyagerConfig::hardcoded();
            tracing::info!("Voyager configuration {:?}", voyager_config);
            let voyager_client = match verification::voyager::VoyagerClient::new(voyager_config) {
                Ok(client) => {
                    tracing::info!("Voyager client initialized and enabled");
                    Some(client)
                }
                Err(e) => {
                    tracing::warn!("Failed to initialize Voyager client: {:?}", e);
                    None
                }
            };

            let shared_state = Arc::new(AppState {
                db_pool,
                s3_client,
                simulation_cache,
                external_class_cache,
                voyager_client,
            });

            let (prometheus_layer, metric_handle) = PrometheusMetricLayer::pair();

            let app = Router::new()
                .route("/dashboard/data", get(get_verification_data))
                .route("/health", get(health_check))
                .route("/v1/simulate-transaction", post(simulate_transaction))
                .route(
                    "/v1/:chain_id/simulate-transaction/:tx_hash",
                    get(simulate_transaction_by_hash_handler),
                )
                .route(
                    "/v1/:chain_id/classes/:class_hash",
                    get(get_class_handler_with_chain_id),
                )
                .route("/v1/classes/:class_hash", get(get_class_handler))
                .route(
                    "/v1/classes/:class_hash/contracts",
                    get(get_contracts_by_class_hash_handler),
                )
                .route("/v1/contracts/:contract_address", get(get_contract_handler))
                .route(
                    "/v1/contracts/:contract_address/entrypoints",
                    get(get_contract_entrypoints_handler),
                )
                .route("/v1/search/:search_hash", get(get_search_handler))
                .route("/v1/:chain_id/verify", post(verify_handler))
                .route("/v1/verify", post(verify_handler_with_rpc))
                .route(
                    "/v1/verification/:verification_status_id/status",
                    get(get_verification_status_handler),
                )
                .route("/v1/debug-transaction", post(debug_transaction))
                .route("/v1/decode-calldata", post(decode_calldata_handler))
                // .route("/v1/cache/stats", get(cache_stats_handler)) // Commented out for now
                .with_state(shared_state)
                .route("/metrics", get(|| async move { metric_handle.render() }))
                .route_service(
                    "/",
                    axum::routing::get(|| async { axum::response::Json(ApiDoc::openapi()) }),
                )
                .layer(tower_http::trace::TraceLayer::new_for_http())
                .layer(sentry_tower::NewSentryLayer::<axum::http::Request<_>>::new_from_top())
                .layer(prometheus_layer)
                .route("/_ah/warmup", get(|| async { "OK" }))
                .layer(CorsLayer::permissive());

            println!("Listening on 0.0.0.0:3000");
            axum::Server::bind(&"0.0.0.0:3000".parse().unwrap())
                .serve(app.into_make_service())
                .await
                .unwrap();

            Ok(())
        })
}

// If DB is down SQLX query is hanging, this is why 3 secs timeout
async fn health_check(State(state): State<Arc<AppState>>) -> StatusCode {
    let db_status = match timeout(
        Duration::from_secs(3),
        sqlx::query("SELECT 1").execute(&state.db_pool),
    )
    .await
    {
        db_status => match db_status {
            Ok(_) => StatusCode::OK,
            Err(_) => StatusCode::INTERNAL_SERVER_ERROR,
        },
    };
    // If the database is down, we should return an error
    db_status
}
