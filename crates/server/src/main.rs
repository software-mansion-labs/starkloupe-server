mod app_state;
mod db;
mod handlers;

extern crate dotenv;

use app_state::AppState;
use axum::{
    extract::{Path, State},
    http::Request,
    http::StatusCode,
    middleware::{self, Next},
    response::Response,
    routing::get,
    routing::post,
    Json, Router,
};
use dotenv::dotenv;
use handlers::simulate::simulate;
use sqlx::postgres::PgPoolOptions;
use std::sync::Arc;

// Resources
// https://github.com/tokio-rs/axum/tree/main/examples
// https://crates.io/crates/redis-macros
// https://www.apianalytics.dev/
// - https://github.com/tom-draper/api-analytics
// https://docs.rs/axum-prometheus/latest/axum_prometheus/

async fn auth_middleware<B>(
    State(_state): State<Arc<AppState>>,
    req: Request<B>,
    next: Next<B>,
) -> Result<Response, StatusCode> {
    let is_authorized = match req.headers().get("x-api-key") {
        Some(key) if key == "your_api_key" => true,
        _ => false,
    };

    if !is_authorized {
        return Err(StatusCode::UNAUTHORIZED);
    }

    Ok(next.run(req).await)
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    dotenv().ok();

    // let redis_addr = std::env::var("REDIS_ADDR").unwrap_or("redis://127.0.0.1/".to_string());
    let db_addr = std::env::var("DATABASE_URL").unwrap_or("postgres://".to_string());
    let pool = PgPoolOptions::new()
        .max_connections(2)
        .connect(&db_addr)
        .await?;
    // let client = redis::Client::open(redis_addr)?;

    sqlx::migrate!().run(&pool).await?;

    let shared_state = Arc::new(AppState {
        db_pool: pool,
        // redis_client: Arc::new(client),
    });

    let app = Router::new()
        .route("/simulate", post(simulate))
        .route_layer(middleware::from_fn_with_state(
            shared_state.clone(),
            auth_middleware,
        ))
        .route("/:chain/tx/:hash", get(read_transaction))
        .with_state(shared_state);

    println!("Listening on 0.0.0.0:3000");

    axum::Server::bind(&"0.0.0.0:3000".parse().unwrap())
        .serve(app.into_make_service())
        .await
        .unwrap();

    Ok(())
}

async fn read_transaction(
    State(_state): State<Arc<AppState>>,
    path: Path<(String, String)>,
) -> Result<Json<String>, StatusCode> {
    // Implement your business logic here
    dbg!(path);
    Ok(Json("Hello, World!".to_string()))
}
