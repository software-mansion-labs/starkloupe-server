mod app_state;
mod config;
mod handlers;

extern crate dotenv;

use app_state::AppState;
use axum::{
    body::Body,
    extract::{Path, State},
    http::{header, HeaderValue, Method, Request, StatusCode},
    middleware::{self, Next},
    response::{IntoResponse, Response},
    routing::get,
    routing::post,
    Json, Router,
};
use axum_prometheus::PrometheusMetricLayer;
use config::rpc_url;
use db::Project;
use deadpool_redis;
use dotenv::dotenv;
use handlers::{
    auth::{cache_all_users_and_projects, user_auth_middleware},
    simulate::simulate_handler,
    simulate_trace::{simulate_trace, simulate_transaction},
    simulations::get_simulations,
};
use sqlx::postgres::PgPoolOptions;
use std::sync::Arc;
use tokio::time::Duration;
use tower_http::cors::CorsLayer;
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
    // TODO: We will get project from the DB.
    if req.method() != Method::OPTIONS {
        let project = match req.headers().get("x-api-key") {
            Some(key) => {
                if key == "walnut_ZFqJep8VrMB_LfUXdSeKxJAxNz9AC6rdLK" {
                    // Walnut Project
                    Ok(Project {
                        id: 1,
                        name: String::from("Walnut"),
                        slug: String::from("walnut"),
                    })
                } else if key == "walnut_YPuxeJ7eMTX_8yfAjTjfVvv3K1dyaRdZJF"
                    || key == "walnut_9tkxeupzdAj_8K1zPzun4QaFaiGFQvZhmT"
                {
                    // Briq Project
                    Ok(Project {
                        id: 2,
                        name: String::from("Briq"),
                        slug: String::from("briq"),
                    })
                } else if key == "walnut_6mV1ro7dfrR_HmKxouxqXfVoSy37ip1caz" {
                    // Jediswap
                    Ok(Project {
                        id: 3,
                        name: String::from("Jediswap"),
                        slug: String::from("jediswap"),
                    })
                } else if key == "walnut_LSBhhfrvdhy_CJUpRxe2hA7QHmPUMqhp33" {
                    // Starknet Id
                    Ok(Project {
                        id: 4,
                        name: String::from("Starknet Id"),
                        slug: String::from("starknet-id"),
                    })
                } else if key == "walnut_NbiV2gLJ2yS_XPNHFEg51bMzYH2psq4chs" {
                    // HH India: Satyam Bansal (@satyambnsal)
                    Ok(Project {
                        id: 5,
                        name: String::from("@satyambnsal"),
                        slug: String::from("satyambnsal"),
                    })
                } else if key == "walnut_Pqz5bFL2wSb_9uQZXpBXgLqEPZHTz04QzN" {
                    // Carmine
                    Ok(Project {
                        id: 6,
                        name: String::from("Carmine"),
                        slug: String::from("carmine"),
                    })
                } else if key == "walnut_64vz74v5zPb_osGq4TZSEW3jD8DoK2TJx4" {
                    Ok(Project {
                        id: 7,
                        name: String::from("LayerAkira"),
                        slug: String::from("layerakira"),
                    })
                } else {
                    Err(StatusCode::UNAUTHORIZED)
                }
            }
            _ => Err(StatusCode::UNAUTHORIZED),
        }?;

        req.extensions_mut().insert(project);
    }

    Ok(next.run(req).await)
}

async fn forward_post_request(url: &str, req: Request<Body>) -> impl IntoResponse {
    let client = reqwest::Client::new();
    let res = client.post(url).body(req.into_body()).send().await.unwrap();
    let bytes = res.bytes().await.unwrap();

    let mut response: Response<Body> = Response::new(bytes.into());
    response.headers_mut().insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("application/json"),
    );

    response
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    dotenv().ok();

    tracing_subscriber::registry()
        .with(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "info,server=debug,tower_http=debug".into()),
        )
        .with(tracing_subscriber::fmt::layer().json())
        .init();

    let redis_addr = std::env::var("REDIS_ADDR").unwrap_or("redis://127.0.0.1/".to_string());
    let db_addr = std::env::var("DATABASE_URL").unwrap_or("postgres://".to_string());
    let db_pool = PgPoolOptions::new()
        .max_connections(2)
        .connect(&db_addr)
        .await?;

    let redis_cfg = deadpool_redis::Config::from_url(redis_addr);
    let redis_pool = redis_cfg
        .create_pool(Some(deadpool_redis::Runtime::Tokio1))
        .unwrap();

    let db_pool_for_background = db_pool.clone();
    let redis_pool_for_background = redis_pool.clone();

    sqlx::migrate!().run(&db_pool).await?;

    // Schedule background task that runs refetch every 5 minutes
    // TODO: Cross task concurrency issues exist here, but it's fine for now.
    tokio::spawn(async move {
        loop {
            let _ = cache_all_users_and_projects(
                &redis_pool_for_background,
                &db_pool_for_background,
                60 * 5,
            )
            .await;
            tokio::time::sleep(Duration::from_secs((60 * 5) - 30)).await;
        }
    });

    let shared_state = Arc::new(AppState {
        db_pool,
        redis_pool,
    });

    let (prometheus_layer, metric_handle) = PrometheusMetricLayer::pair();

    let user_auth_routes = Router::new()
        .route("/v1/simulations", get(get_simulations))
        .layer(middleware::from_fn_with_state(
            shared_state.clone(),
            user_auth_middleware,
        ))
        .layer(CorsLayer::permissive());

    let app = Router::new()
        .route("/v1/simulate", post(simulate_handler))
        .route_layer(middleware::from_fn_with_state(
            shared_state.clone(),
            auth_middleware,
        ))
        .merge(user_auth_routes)
        .route("/v1/:chain/tx/:hash", get(read_transaction))
        .route("/v1/simulate-transaction", post(simulate_transaction))
        .route("/v1/simulate-trace/:id", get(simulate_trace))
        .route("/_ah/warmup", get(|| async { "OK" }))
        .with_state(shared_state)
        .route("/metrics", get(|| async move { metric_handle.render() }))
        .route(
            "/rpc/0x534e5f474f45524c49",
            post(|req| forward_post_request(rpc_url("0x534e5f474f45524c49"), req)),
        )
        .route(
            "/rpc/0x534e5f4d41494e",
            post(|req| forward_post_request(rpc_url("0x534e5f4d41494e"), req)),
        )
        .layer(prometheus_layer)
        .layer(tower_http::trace::TraceLayer::new_for_http())
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
