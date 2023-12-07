mod app_state;
mod db;
mod handlers;
mod utils;

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
use axum_prometheus::PrometheusMetricLayer;
use cookie::Cookie;
use db::{Project, User};
use dotenv::dotenv;
use handlers::{simulate::simulate, simulate_trace::simulate_trace, simulations::get_simulations};
use jsonwebtoken::{decode, Algorithm, DecodingKey, Validation};
use serde::{Deserialize, Serialize};
use sqlx::postgres::PgPoolOptions;
use std::sync::Arc;
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
                    Ok(Project { id: 1 })
                } else if key == "walnut_YPuxeJ7eMTX_8yfAjTjfVvv3K1dyaRdZJF"
                    || key == "walnut_9tkxeupzdAj_8K1zPzun4QaFaiGFQvZhmT"
                {
                    // Briq Project
                    Ok(Project { id: 2 })
                } else if key == "walnut_6mV1ro7dfrR_HmKxouxqXfVoSy37ip1caz" {
                    // Jediswap
                    Ok(Project { id: 3 })
                } else if key == "walnut_LSBhhfrvdhy_CJUpRxe2hA7QHmPUMqhp33" {
                    // Starknet Id
                    Ok(Project { id: 4 })
                } else if key == "walnut_NbiV2gLJ2yS_XPNHFEg51bMzYH2psq4chs" {
                    // HH India: Satyam Bansal (@satyambnsal)
                    Ok(Project { id: 5 })
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

#[derive(Debug, Serialize, Deserialize)]
struct Claims {
    email: String,
    sub: String,
    projects: Vec<String>,
    iat: i32,
}

async fn user_auth_middleware<B>(
    State(_state): State<Arc<AppState>>,
    mut req: Request<B>,
    next: Next<B>,
) -> Result<Response, StatusCode> {
    if req.method() != Method::OPTIONS {
        let session_token = if let Some(header_value) = req.headers().get("Authorization") {
            if let Ok(auth_str) = header_value.to_str() {
                if auth_str.starts_with("Bearer ") {
                    Some(auth_str.trim_start_matches("Bearer "))
                } else {
                    return Err(StatusCode::UNAUTHORIZED);
                }
            } else {
                return Err(StatusCode::UNAUTHORIZED);
            }
        } else if let Some(cookie_value) = req.headers().get("cookie") {
            // Parse cookies and look for "session-token"
            if let Ok(cookie_str) = cookie_value.to_str() {
                for cookie in Cookie::split_parse_encoded(cookie_str) {
                    if let Ok(cookie) = cookie {
                        if cookie.name() == "session-token" {
                            Some(cookie.value());
                        }
                    }
                }
            }
            return Err(StatusCode::UNAUTHORIZED);
        } else {
            return Err(StatusCode::UNAUTHORIZED);
        };

        if let Some(token) = session_token {
            if let Ok(data) = decode::<Claims>(
                token,
                &DecodingKey::from_secret(
                    b"cc7e0d44fd473002f1c42167459001140ec6389b7353f8088f4d9a95f2f596f2",
                ),
                &Validation::new(Algorithm::HS256),
            ) {
                let claims = data.claims;
                req.extensions_mut().insert(User { sub: claims.sub });

                let projects_result = sqlx::query!(
                    r#"
                    SELECT * FROM projects
                    WHERE slug = ANY($1)
                    "#,
                    &claims.projects as _
                )
                .fetch_all(&_state.db_pool)
                .await;

                match projects_result {
                    Ok(rows) => {
                        let projects: Vec<Project> =
                            rows.into_iter().map(|row| Project { id: row.id }).collect();
                        req.extensions_mut().insert(projects);
                    }
                    Err(e) => {
                        dbg!(e);
                        // Handle the error, e.g., log it or return it
                    }
                }
            } else {
                return Err(StatusCode::UNAUTHORIZED);
            }
        }
    }

    Ok(next.run(req).await)
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

    let (prometheus_layer, metric_handle) = PrometheusMetricLayer::pair();

    let user_auth_routes = Router::new()
        .route("/v1/simulations", get(get_simulations))
        .layer(middleware::from_fn_with_state(
            shared_state.clone(),
            user_auth_middleware,
        ));

    let app = Router::new()
        .route("/v1/simulate", post(simulate))
        .route_layer(middleware::from_fn_with_state(
            shared_state.clone(),
            auth_middleware,
        ))
        .merge(user_auth_routes)
        .route("/v1/:chain/tx/:hash", get(read_transaction))
        .route("/v1/simulate-trace/:id", get(simulate_trace))
        .route("/_ah/warmup", get(|| async { "OK" }))
        .with_state(shared_state)
        .route("/metrics", get(|| async move { metric_handle.render() }))
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
