mod app_state;
mod db;
mod handlers;

extern crate dotenv;

use app_state::AppState;
use axum::{
    extract::{Path, State},
    http::Method,
    http::Request,
    http::StatusCode,
    middleware::{self, Next},
    response::Response,
    routing::get,
    routing::post,
    Json, Router,
};
use db::Team;
use dotenv::dotenv;
use handlers::{simulate::simulate, simulations::get_simulations};
use sqlx::postgres::PgPoolOptions;
use std::sync::Arc;
use tower_http::cors::{Any, CorsLayer};
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt};

// Resources
// https://github.com/tokio-rs/axum/tree/main/examples
// https://crates.io/crates/redis-macros
// https://www.apianalytics.dev/
// - https://github.com/tom-draper/api-analytics
// https://docs.rs/axum-prometheus/latest/axum_prometheus/

async fn auth_middleware<B>(
    State(_state): State<Arc<AppState>>,
    mut req: Request<B>,
    next: Next<B>,
) -> Result<Response, StatusCode> {
    // TODO: We will get team from the DB.
    if req.method() != Method::OPTIONS {
        let team = match req.headers().get("x-api-key") {
            Some(key) => {
                if key == "walnut_ZFqJep8VrMB_LfUXdSeKxJAxNz9AC6rdLK" {
                    // Walnut Team
                    Ok(Team { id: 1 })
                } else if key == "walnut_YPuxeJ7eMTX_8yfAjTjfVvv3K1dyaRdZJF"
                    || key == "walnut_9tkxeupzdAj_8K1zPzun4QaFaiGFQvZhmT"
                {
                    // Briq Team
                    Ok(Team { id: 2 })
                } else {
                    Err(StatusCode::UNAUTHORIZED)
                }
            }
            _ => Err(StatusCode::UNAUTHORIZED),
        }?;

        req.extensions_mut().insert(team);
    }

    Ok(next.run(req).await)
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    dotenv().ok();

    tracing_subscriber::registry()
        .with(tracing_subscriber::EnvFilter::new("debug"))
        // .with(
        //     tracing_subscriber::EnvFilter::try_from_default_env()
        //         .unwrap_or_else(|_| "server=debug".into()),
        // )
        .with(tracing_subscriber::fmt::layer())
        .init();

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
        .route("/v1/simulate", post(simulate))
        .route("/v1/simulations", get(get_simulations))
        .route_layer(middleware::from_fn_with_state(
            shared_state.clone(),
            auth_middleware,
        ))
        .route("/v1/:chain/tx/:hash", get(read_transaction))
        .route("/_ah/warmup", get(|| async { "OK" }))
        .with_state(shared_state)
        .layer(CorsLayer::permissive());

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
