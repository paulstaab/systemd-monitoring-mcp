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
        if let Some(forwarded_for) = request
            .headers()
            .get("x-forwarded-for")
            .and_then(|v| v.to_str().ok())
        {
            // X-Forwarded-For is a comma-separated list; the left-most entry is the original client.
            if let Some(first) = forwarded_for.split(',').next() {
                if let Ok(ip) = first.trim().parse::<IpAddr>() {
                    return Ok(ip);
                }
            }
        }
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
    use super::{parse_bearer_token, tokens_match};

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
}
