mod app_state;
mod handlers;

extern crate dotenv;

use app_state::AppState;
use aws_config::meta::region::RegionProviderChain;
use axum::{routing::get, routing::post, Router};
use axum_prometheus::PrometheusMetricLayer;
use deadpool_redis;
use dotenv::dotenv;
use handlers::{
    classes::get_class_handler,
    openapi::ApiDoc,
    simulate::{simulate_transaction, simulate_transaction_by_hash_handler},
    verification::verify_handler,
};
use sqlx::postgres::PgPoolOptions;
use std::sync::Arc;
use tower_http::cors::CorsLayer;
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt};
use utoipa::OpenApi;

use crate::handlers::{
    classes::get_class_handler_with_chain_id,
    contracts::get_contract_handler_with_chain_id,
    search::get_search_handler,
    verification::{get_verification_status_handler, verify_handler_with_rpc},
};

use sentry;

// Resources
// https://github.com/tokio-rs/axum/tree/main/examples
// https://crates.io/crates/redis-macros
// https://www.apianalytics.dev/
// - https://github.com/tom-draper/api-analytics
// https://docs.rs/axum-prometheus/latest/axum_prometheus/

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    dotenv().ok();

    let _guard = sentry::init(("https://ae2d01aafee9ea77f4090092df5a6a42@o4507958254436352.ingest.us.sentry.io/4507961681838080", sentry::ClientOptions {
        release: sentry::release_name!(),
        sample_rate: 1.0,
        ..sentry::ClientOptions::default()
    }));

    tracing_subscriber::registry()
        .with(tracing_subscriber::EnvFilter::new("info"))
        .with(tracing_subscriber::fmt::layer())
        .with(sentry_tracing::layer())
        .init();

    let redis_addr = std::env::var("REDIS_ADDR").unwrap_or("redis://127.0.0.1/".to_string());
    let db_addr = std::env::var("DATABASE_URL").unwrap_or("postgres://".to_string());
    let db_pool = PgPoolOptions::new()
        .max_connections(30)
        .connect(&db_addr)
        .await?;

    let redis_cfg = deadpool_redis::Config::from_url(redis_addr);
    let redis_pool = redis_cfg
        .create_pool(Some(deadpool_redis::Runtime::Tokio1))
        .unwrap();

    let region_provider = RegionProviderChain::default_provider();
    let shared_config = aws_config::from_env().region(region_provider).load().await;
    let s3_client = aws_sdk_s3::Client::new(&shared_config);

    sqlx::migrate!().run(&db_pool).await?;

    let shared_state = Arc::new(AppState {
        db_pool,
        redis_pool,
        s3_client,
    });

    let (prometheus_layer, metric_handle) = PrometheusMetricLayer::pair();

    let app = Router::new()
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
        .route(
            "/v1/:chain_id/contracts/:contract_address",
            get(get_contract_handler_with_chain_id),
        )
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
}
