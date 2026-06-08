// Admin-token auth extractor; not wired up to any route yet (used by M1).
#![allow(dead_code)]

use axum::{
    async_trait,
    extract::FromRequestParts,
    http::{request::Parts, HeaderMap, StatusCode},
};
use subtle::ConstantTimeEq;

use crate::app_state::AppState;

const HEADER_NAME: &str = "x-admin-token";
const ENV_VAR: &str = "WALNUT_ADMIN_TOKEN";

#[derive(Debug)]
pub struct AdminAuth;

fn verify_admin_token(headers: &HeaderMap) -> Result<(), StatusCode> {
    let header = headers.get(HEADER_NAME).ok_or(StatusCode::UNAUTHORIZED)?;
    let expected = std::env::var(ENV_VAR).map_err(|_| StatusCode::UNAUTHORIZED)?;

    if bool::from(header.as_bytes().ct_eq(expected.as_bytes())) {
        Ok(())
    } else {
        Err(StatusCode::UNAUTHORIZED)
    }
}

#[async_trait]
impl FromRequestParts<AppState> for AdminAuth {
    type Rejection = StatusCode;

    async fn from_request_parts(
        parts: &mut Parts,
        _state: &AppState,
    ) -> Result<Self, Self::Rejection> {
        verify_admin_token(&parts.headers).map(|_| AdminAuth)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::http::{HeaderName, HeaderValue};

    fn headers_with(pairs: &[(&'static str, &str)]) -> HeaderMap {
        let mut map = HeaderMap::new();
        for (k, v) in pairs {
            map.insert(
                HeaderName::from_static(k),
                HeaderValue::from_str(v).unwrap(),
            );
        }
        map
    }

    #[test]
    fn admin_auth_rejects_missing_header() {
        std::env::set_var(ENV_VAR, "secret-correct-token");
        assert_eq!(
            verify_admin_token(&headers_with(&[])).unwrap_err(),
            StatusCode::UNAUTHORIZED
        );
    }

    #[test]
    fn admin_auth_rejects_wrong_value() {
        std::env::set_var(ENV_VAR, "secret-correct-token");
        assert_eq!(
            verify_admin_token(&headers_with(&[("x-admin-token", "wrong")])).unwrap_err(),
            StatusCode::UNAUTHORIZED
        );
    }

    #[test]
    fn admin_auth_accepts_correct_value() {
        std::env::set_var(ENV_VAR, "secret-correct-token");
        assert!(
            verify_admin_token(&headers_with(&[("x-admin-token", "secret-correct-token")])).is_ok()
        );
    }
}
