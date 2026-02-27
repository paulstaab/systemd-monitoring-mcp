use std::net::IpAddr;

use axum::{
    extract::connect_info::ConnectInfo,
    extract::{Request, State},
    http::header,
    middleware::Next,
    response::Response,
};
use hmac::{Hmac, Mac};
use sha2::Sha256;

use crate::{errors::AppError, AppState};

type HmacSha256 = Hmac<Sha256>;

/// Constant-time token comparison using HMAC: both sides produce HMAC(key, msg="mcp-token-verify")
/// and verify using `Mac::verify_slice` which is constant-time internally.
fn tokens_match(expected: &str, provided: &str) -> bool {
    let Ok(mut mac) = HmacSha256::new_from_slice(expected.as_bytes()) else {
        return false;
    };
    mac.update(b"mcp-token-verify");

    let Ok(mut provided_mac) = HmacSha256::new_from_slice(provided.as_bytes()) else {
        return false;
    };
    provided_mac.update(b"mcp-token-verify");
    let provided_tag = provided_mac.finalize().into_bytes();

    // `verify_slice` uses constant-time comparison via the `subtle` crate internally.
    mac.verify_slice(&provided_tag).is_ok()
}

pub async fn require_bearer_token(
    State(state): State<AppState>,
    request: Request,
    next: Next,
) -> Result<Response, AppError> {
    let header_value = request
        .headers()
        .get(header::AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        .ok_or_else(|| AppError::unauthorized("missing_token", "missing authorization header"))?;

    let provided_token = parse_bearer_token(header_value)
        .ok_or_else(|| AppError::unauthorized("invalid_token", "invalid authorization scheme"))?;

    if !tokens_match(state.api_token.as_ref(), provided_token) {
        return Err(AppError::unauthorized(
            "invalid_token",
            "invalid bearer token",
        ));
    }

    Ok(next.run(request).await)
}

pub async fn enforce_ip_allowlist(
    State(state): State<AppState>,
    request: Request,
    next: Next,
) -> Result<Response, AppError> {
    if let Some(allowed_cidr) = state.allowed_cidr.as_ref() {
        let client_ip = extract_client_ip(&state, &request)?;

        if !allowed_cidr.contains(&client_ip) {
            return Err(AppError::forbidden(
                "ip_restricted",
                "request source IP is not allowed",
            ));
        }
    }

    Ok(next.run(request).await)
}

/// Extract the client IP, respecting X-Forwarded-For when the direct peer is a trusted proxy.
fn extract_client_ip(state: &AppState, request: &Request) -> Result<IpAddr, AppError> {
    let connect_info = request
        .extensions()
        .get::<ConnectInfo<std::net::SocketAddr>>()
        .ok_or_else(|| {
            AppError::forbidden(
                "ip_restricted",
                "request source IP is unavailable for allowlist validation",
            )
        })?;
    let peer_ip = connect_info.0.ip();

    // Only trust forwarded headers when the direct peer is in the trusted proxy list.
    let peer_is_trusted = state
        .trusted_proxies
        .iter()
        .any(|cidr| cidr.contains(&peer_ip));

    if peer_is_trusted {
        let forwarded_for = request
            .headers()
            .get("x-forwarded-for")
            .and_then(|v| v.to_str().ok())
            .ok_or_else(|| {
                AppError::forbidden(
                    "ip_restricted",
                    "x-forwarded-for is required when request comes from a trusted proxy",
                )
            })?;

        // X-Forwarded-For is a comma-separated list; the left-most entry is the original client.
        let first = forwarded_for.split(',').next().ok_or_else(|| {
            AppError::forbidden(
                "ip_restricted",
                "x-forwarded-for is required when request comes from a trusted proxy",
            )
        })?;

        let forwarded_ip = first.trim().parse::<IpAddr>().map_err(|_| {
            AppError::forbidden(
                "ip_restricted",
                "x-forwarded-for contains an invalid client IP",
            )
        })?;

        return Ok(forwarded_ip);
    }

    Ok(peer_ip)
}

fn parse_bearer_token(value: &str) -> Option<&str> {
    value
        .strip_prefix("Bearer ")
        .filter(|token| !token.is_empty())
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use axum::{
        extract::connect_info::ConnectInfo,
        http::{header, Request},
    };

    use crate::{errors::AppError, systemd_client::DbusSystemdClient, AppState};

    use super::{extract_client_ip, parse_bearer_token, tokens_match};

    fn state_with_trusted_proxies(trusted_proxies: &[&str]) -> AppState {
        AppState::new(
            "abcdefghijklmnop".to_string(),
            None,
            trusted_proxies
                .iter()
                .map(|cidr| cidr.parse().expect("valid cidr"))
                .collect(),
            Arc::new(DbusSystemdClient::new()),
        )
    }

    fn request_with_peer_and_optional_xff(
        peer_ip: [u8; 4],
        xff: Option<&str>,
    ) -> Request<axum::body::Body> {
        let mut builder = Request::builder().uri("/logs").method("GET");
        if let Some(xff_value) = xff {
            builder = builder.header(
                header::HeaderName::from_static("x-forwarded-for"),
                xff_value,
            );
        }
        let mut request = builder
            .body(axum::body::Body::empty())
            .expect("request build");
        request
            .extensions_mut()
            .insert(ConnectInfo(std::net::SocketAddr::from((peer_ip, 9000))));
        request
    }

    #[test]
    fn parses_bearer_token() {
        assert_eq!(parse_bearer_token("Bearer token"), Some("token"));
        assert_eq!(parse_bearer_token("Basic token"), None);
    }

    #[test]
    fn constant_time_match_works() {
        assert!(tokens_match("my-secret-token", "my-secret-token"));
        assert!(!tokens_match("my-secret-token", "wrong-token"));
        assert!(!tokens_match("my-secret-token", ""));
        // Two empty strings produce the same HMAC — this is expected;
        // the config layer enforces minimum token length so this case
        // never arises in production.
        assert!(tokens_match("", ""));
    }

    #[test]
    fn trusted_peer_with_valid_xff_uses_xff_ip() {
        let state = state_with_trusted_proxies(&["10.0.0.0/8"]);
        let request =
            request_with_peer_and_optional_xff([10, 10, 10, 10], Some("203.0.113.7, 10.0.0.1"));

        let client_ip = extract_client_ip(&state, &request).expect("client ip extraction");

        assert_eq!(
            client_ip,
            "203.0.113.7".parse::<std::net::IpAddr>().expect("valid ip")
        );
    }

    #[test]
    fn untrusted_peer_with_xff_ignores_xff_and_uses_peer_ip() {
        let state = state_with_trusted_proxies(&["10.0.0.0/8"]);
        let request = request_with_peer_and_optional_xff([192, 168, 1, 10], Some("203.0.113.7"));

        let client_ip = extract_client_ip(&state, &request).expect("client ip extraction");

        assert_eq!(
            client_ip,
            "192.168.1.10"
                .parse::<std::net::IpAddr>()
                .expect("valid ip")
        );
    }

    #[test]
    fn trusted_peer_with_missing_xff_is_rejected() {
        let state = state_with_trusted_proxies(&["10.0.0.0/8"]);
        let request = request_with_peer_and_optional_xff([10, 10, 10, 10], None);

        let error = extract_client_ip(&state, &request).expect_err("expected forbidden error");

        assert!(matches!(
            error,
            AppError::Forbidden {
                code: "ip_restricted",
                ..
            }
        ));
    }

    #[test]
    fn trusted_peer_with_invalid_xff_is_rejected() {
        let state = state_with_trusted_proxies(&["10.0.0.0/8"]);
        let request = request_with_peer_and_optional_xff([10, 10, 10, 10], Some("not-an-ip"));

        let error = extract_client_ip(&state, &request).expect_err("expected forbidden error");

        assert!(matches!(
            error,
            AppError::Forbidden {
                code: "ip_restricted",
                ..
            }
        ));
    }
}
