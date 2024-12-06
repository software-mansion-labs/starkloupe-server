extern crate dotenv;
mod app_state;
mod handlers;
mod services;
mod telegram_bot_service;

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
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt};
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

// Resources
// https://github.com/tokio-rs/axum/tree/main/examples
// https://www.apianalytics.dev/
// - https://github.com/tom-draper/api-analytics
// https://docs.rs/axum-prometheus/latest/axum_prometheus/

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    dotenv().ok();

    if !cfg!(debug_assertions) {
        let _guard = sentry::init(("https://ae2d01aafee9ea77f4090092df5a6a42@o4507958254436352.ingest.us.sentry.io/4507961681838080", sentry::ClientOptions {
            release: sentry::release_name!(),
            sample_rate: 1.0,
            ..sentry::ClientOptions::default()
        }));
    }

    tracing_subscriber::registry()
        .with(tracing_subscriber::EnvFilter::new("info"))
        .with(tracing_subscriber::fmt::layer())
        .with(sentry_tracing::layer())
        .init();

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

    download_binary(&s3_client, format!("sozo/{ARCH}/sozo_v1.0.1").as_str()).await?;
    download_binary(&s3_client, format!("scarb/{ARCH}/scarb_cairo_v_2_6_3").as_str()).await?;
    download_binary(&s3_client, format!("scarb/{ARCH}/scarb_cairo_v_2_6_4").as_str()).await?;
    download_binary(&s3_client, format!("scarb/{ARCH}/scarb_cairo_v_2_7_0").as_str()).await?;
    download_binary(&s3_client, format!("scarb/{ARCH}/scarb_cairo_v2.8.2").as_str()).await?;
    download_binary(&s3_client, format!("scarb/{ARCH}/scarb_cairo_v2.8.4").as_str()).await?;
    download_binary(&s3_client, format!("scarb/{ARCH}/scarb_cairo_v2.8.5").as_str()).await?;

    let shared_state = Arc::new(AppState {
        db_pool,
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
}

// downloads the binary from the S3 bucket and saves it to the local directory, gives the executable permissions
async fn download_binary(
    s3_client: &Client,
    object_key: &str,
) -> Result<(), Box<dyn std::error::Error>> {
    let bucket_name = std::env::var("BINARIES_S3_BUCKET_NAME").unwrap_or("".to_string());
    let binaries_save_directory_path = std::env::var("BINARIES_SAVE_DIRECTORY_PATH").unwrap_or("".to_string());
    let local_file_path = format!("{}/{}", &binaries_save_directory_path, &object_key.replace(format!("/{}/", ARCH).as_str(), "/"));
    // Check if the file already exists
    let path = Path::new(&local_file_path);
    if path.exists() {
        println!("File already exists (skipping download): {}", local_file_path);
        return Ok(()); // Exit early if the file exists
    }
    println!("Downloading object: {}/{}", bucket_name, object_key);

    // Fetch the object from the S3 bucket
    let resp = s3_client
        .get_object()
        .bucket(bucket_name)
        .key(object_key)
        .send()
        .await?;

    // Ensure the directory exists
    if let Some(parent_dir) = std::path::Path::new(&local_file_path).parent() {
        fs::create_dir_all(parent_dir).expect("Failed to create parent directories");
    }
    let mut file = File::create(&local_file_path).expect(format!("Failed to create file: {}", local_file_path).as_str());

    // Stream the object content to the file
    let data = resp.body.collect().await?;
    file.write_all(&data.into_bytes())
        .expect("Failed to write object data to file");

    let metadata = fs::metadata(&local_file_path)?;
    let mut permissions = metadata.permissions();
    permissions.set_mode(0o755); // rwxr-xr-x
    fs::set_permissions(&local_file_path, permissions)
        .expect("Failed to set executable permissions");

    println!("Object downloaded successfully to: {}", local_file_path);

    Ok(())
}