extern crate dotenv;

use axum::{extract::Path, response::IntoResponse, routing::get, routing::post, Json, Router};
use dotenv::dotenv;
use serde::{Deserialize, Serialize};
use sqlx::{postgres::PgPoolOptions, Executor, Pool, Postgres};

#[derive(Serialize, Deserialize)]
struct StarkNetTransaction {
    // Define your transaction structure here
}

// Resources
// https://github.com/tokio-rs/axum/tree/main/examples
// https://crates.io/crates/redis-macros
// https://www.apianalytics.dev/
// - https://github.com/tom-draper/api-analytics
// https://docs.rs/axum-prometheus/latest/axum_prometheus/

use std::sync::Arc;

struct AppState {
    db_pool: Pool<Postgres>,
    // redis_client: Arc<redis::Client>,
}

#[derive(Serialize, Deserialize)]
struct Transaction {
    name: String,
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
        // .route(
        // "/simulate",
        // post({
        //     let shared_state1 = Arc::clone(&shared_state);
        //     move |body: Json<Transaction>| simulate(body, shared_state1)
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

async fn simulate(Json(payload): Json<Transaction>, state: Arc<AppState>) {
    let _ = sqlx::query!("INSERT INTO simulations (name) VALUES ($1)", "k")
        .execute(&state.db_pool)
        .await;
}

async fn read_transaction(state: Arc<AppState>, id: Path<String>) -> impl IntoResponse {
    // Implement your business logic here
}
