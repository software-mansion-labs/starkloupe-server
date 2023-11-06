mod app_state;
mod db;
mod handlers;

extern crate dotenv;

use app_state::AppState;
use axum::{extract::Path, response::IntoResponse, routing::get, Router};
use dotenv::dotenv;
use sqlx::postgres::PgPoolOptions;
use std::sync::Arc;

// Resources
// https://github.com/tokio-rs/axum/tree/main/examples
// https://crates.io/crates/redis-macros
// https://www.apianalytics.dev/
// - https://github.com/tom-draper/api-analytics
// https://docs.rs/axum-prometheus/latest/axum_prometheus/

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
        // .route(
        // "/simulate",
        // post({
        //     let shared_state1 = Arc::clone(&shared_state);
        //     move |body: Json<StarkNetTransaction>| simulate(body, shared_state1)
        // })
        .route(
            "/:chain/tx/:hash",
            get({
                let shared_state = Arc::clone(&shared_state);
                move |path: Path<String>| read_transaction(shared_state, path)
            }),
        );

    println!("Listening on 0.0.0.0:3000");

    axum::Server::bind(&"0.0.0.0:3000".parse().unwrap())
        .serve(app.into_make_service())
        .await
        .unwrap();

    Ok(())
}

async fn read_transaction(_state: Arc<AppState>, id: Path<String>) -> impl IntoResponse {
    // Implement your business logic here
    dbg!(id);
    "Hello, World!"
}
