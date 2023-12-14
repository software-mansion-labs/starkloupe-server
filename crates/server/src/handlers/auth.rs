use crate::app_state::AppState;
use crate::db::{Project, User};
use axum::{
    extract::State, http::Method, http::Request, http::StatusCode, middleware::Next,
    response::Response,
};
use cookie::Cookie;
use jsonwebtoken::{decode, Algorithm, DecodingKey, Validation};
use redis;
use redis::AsyncCommands;
use serde::{Deserialize, Serialize};
use sqlx::{Pool, Postgres};
use std::collections::HashMap;
use std::sync::Arc;
use urlencoding::encode;

async fn fetch_users_and_projects(db_pool: &Pool<Postgres>) -> HashMap<String, Vec<Project>> {
    let recs = (sqlx::query!(
        r#"
        SELECT p.*, u.email
        FROM users u
        LEFT JOIN users_projects up ON u.id = up.user_id
        LEFT JOIN projects p ON up.project_id = p.id
        "#
    )
    .fetch_all(db_pool)
    .await)
        .unwrap();

    let mut users_projects_map: HashMap<String, Vec<Project>> = HashMap::new();

    for rec in recs {
        let user_email = rec.email.unwrap();

        if let Some(project_id) = rec.id {
            let project = Project {
                id: project_id,
                name: rec.name.unwrap(),
            };
            users_projects_map
                .entry(user_email)
                .or_insert_with(Vec::new)
                .push(project);
        } else {
            // Ensure that every user is in the map, even if they have no projects
            users_projects_map
                .entry(user_email)
                .or_insert_with(Vec::new);
        }
    }

    users_projects_map
}

fn user_cache_key(user_email: &str) -> String {
    format!("user:{}", encode(user_email))
}

pub async fn cache_all_users_and_projects(
    redis_pool: &deadpool_redis::Pool,
    db_pool: &Pool<Postgres>,
    expiry: i32,
) {
    let mut redis_connection = redis_pool.get().await.unwrap();
    let all_users_projects = fetch_users_and_projects(db_pool).await;

    let mut pipe = redis::pipe();

    for (user_email, projects) in all_users_projects {
        let serialized_projects = serde_json::to_string(&projects).unwrap();
        pipe.cmd("SETEX")
            .arg(user_cache_key(&user_email))
            .arg(expiry)
            .arg(&serialized_projects);
    }

    let _: () = pipe.query_async(&mut redis_connection).await.unwrap();
}

async fn get_user_projects(
    redis_conn: &mut deadpool_redis::Connection,
    user_email: &String,
) -> Vec<Project> {
    let cached_value: Option<String> = redis::cmd("GET")
        .arg(user_cache_key(&user_email))
        .query_async(redis_conn)
        .await
        .unwrap();

    match cached_value {
        Some(value) => {
            let user_projects: Vec<Project> = serde_json::from_str(&value).unwrap();
            user_projects
        }
        None => Vec::new(),
    }
}

#[derive(Debug, Serialize, Deserialize)]
struct Claims {
    email: String,
    sub: String,
    iat: i32,
}

pub async fn user_auth_middleware<B>(
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
                let user_email = claims.email;

                let mut redis_connection = _state.redis_pool.get().await.unwrap();
                let user_projects = get_user_projects(&mut redis_connection, &user_email).await;

                req.extensions_mut().insert(User { email: user_email });

                req.extensions_mut().insert(user_projects);
            } else {
                return Err(StatusCode::UNAUTHORIZED);
            }
        }
    }

    Ok(next.run(req).await)
}
