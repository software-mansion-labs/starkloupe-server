extern crate dotenv;
mod app_state;
mod handlers;
mod services;
mod telegram_bot_service;
mod binaries_manager_service;

use app_state::AppState;
use aws_config::meta::region::RegionProviderChain;
use aws_sdk_s3::config::Region;
use aws_sdk_s3::Client;
use axum::{routing::get, routing::post, Router};
use axum_prometheus::PrometheusMetricLayer;
use dotenv::dotenv;
use handlers::{
    classes::get_class_handler,
    contracts::get_contract_handler,
    openapi::ApiDoc,
    simulate::{simulate_transaction, simulate_transaction_by_hash_handler},
    verification::verify_handler,
};
use sqlx::postgres::PgPoolOptions;
use std::error::Error;
use std::fs::File;
use std::io::Write;
use std::sync::Arc;
use tower_http::cors::CorsLayer;
use tracing_subscriber::{fmt, layer::SubscriberExt, util::SubscriberInitExt, EnvFilter};
use utoipa::OpenApi;

use crate::handlers::{
    classes::get_class_handler_with_chain_id,
    search::get_search_handler,
    verification::{get_verification_status_handler, verify_handler_with_rpc},
};
use sentry;
use std::env::consts::ARCH;
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use axum::extract::State;
use axum::http::StatusCode;
use clokwerk::{Job, AsyncScheduler, TimeUnits};
use tokio::spawn;
use tokio::time::{interval, timeout, Duration};
use tracing::{error, info};
use crate::binaries_manager_service::{download_scarb_and_sozo_binaries_from_s3, start_github_dojo_binaries_downloader_scheduler, start_github_scarb_binaries_downloader_scheduler};
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
        .with(EnvFilter::new(std::env::var("LOG_LEVEL").unwrap_or("INFO".to_string()))) // Set the maximum log level to INFO
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

            let shared_state = Arc::new(AppState {
                db_pool,
                s3_client,
            });

            let (prometheus_layer, metric_handle) = PrometheusMetricLayer::pair();

            let app = Router::new()
                .route("/health", get(health_check))
                .route("/v1/simulate-transaction", post(simulate_transaction))
                .route(
                    "/v1/:chain_id/simulate-transaction/:tx_hash",
                    get(simulate_transaction_by_hash_handler),
                )
                .route("/v1/:chain_id/verify", post(verify_handler))
                .route("/v1/verify", post(verify_handler_with_rpc))
                .route(
                    "/v1/:chain_id/classes/:class_hash",
                    get(get_class_handler_with_chain_id),
                )
                .route("/v1/classes/:class_hash", get(get_class_handler))
                .route("/v1/contracts/:contract_address", get(get_contract_handler))
                .route("/v1/search/:search_hash", get(get_search_handler))
                .route(
                    "/v1/verification/:verification_status_id/status",
                    get(get_verification_status_handler),
                )
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
    let db_status = match timeout(Duration::from_secs(3), sqlx::query("SELECT 1").execute(&state.db_pool)).await  {
        db_status =>
            match db_status {
                Ok(_) => StatusCode::OK,
                Err(_) => StatusCode::INTERNAL_SERVER_ERROR,
            },
    };
    // If the database is down, we should return an error
    db_status
}
