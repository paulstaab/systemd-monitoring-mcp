use axum::{
    extract::{Request, State},
    http::header,
    http::HeaderValue,
    middleware::Next,
    response::Response,
};
use axum_extra::headers::{authorization::Bearer, Authorization, Header};
use hmac::{Hmac, Mac};
use sha2::Sha256;

use crate::{errors::AppError, AppState};

type HmacSha256 = Hmac<Sha256>;
const AUTH_COMPARE_KEY: &[u8] = b"systemd-monitoring-mcp bearer token comparison";

/// Auth middleware that enforces Bearer token access to protected MCP routes.
///
/// Rejects missing or malformed authorization headers with stable auth errors.
/// This function does not log token values and forwards the request only after
/// an HMAC-based constant-time token match against configured app state.
pub async fn require_bearer_token(
    State(state): State<AppState>,
    request: Request,
    next: Next,
) -> Result<Response, AppError> {
    let Some(raw_authorization_header) = request.headers().get(header::AUTHORIZATION) else {
        return Err(AppError::unauthorized(
            "missing_token",
            "missing authorization header",
        ));
    };

    let auth = decode_bearer_authorization(raw_authorization_header)
        .ok_or_else(|| AppError::unauthorized("invalid_token", "invalid authorization scheme"))?;

    if !bearer_token_matches(auth.token(), state.api_token.as_ref()) {
        return Err(AppError::unauthorized(
            "invalid_token",
            "invalid bearer token",
        ));
    }

    Ok(next.run(request).await)
}

/// Decodes a raw HTTP `Authorization` header into a Bearer authorization value.
///
/// Returns `None` when the scheme or structure is not Bearer-compatible.
fn decode_bearer_authorization(value: &HeaderValue) -> Option<Authorization<Bearer>> {
    let mut header_values = std::iter::once(value);
    Authorization::<Bearer>::decode(&mut header_values).ok()
}

/// Compares bearer tokens using fixed-size HMAC tags.
///
/// Both the configured token and the supplied token are first reduced to
/// SHA-256 HMAC tags, then compared with the `hmac` crate's constant-time
/// verification. The static comparison key is not a secret; it keeps the
/// equality check fixed-width so token mismatches do not short-circuit on the
/// first differing byte.
fn bearer_token_matches(provided: &str, expected: &str) -> bool {
    let expected_tag = token_hmac(expected);
    let mut provided_mac =
        HmacSha256::new_from_slice(AUTH_COMPARE_KEY).expect("static HMAC key is valid");
    provided_mac.update(provided.as_bytes());
    provided_mac.verify_slice(&expected_tag).is_ok()
}

/// Computes the fixed-size authentication comparison tag for a token string.
///
/// The return value is only used inside `bearer_token_matches`; callers should
/// never log or expose it because it is derived from bearer credential material.
fn token_hmac(token: &str) -> Vec<u8> {
    let mut mac = HmacSha256::new_from_slice(AUTH_COMPARE_KEY).expect("static HMAC key is valid");
    mac.update(token.as_bytes());
    mac.finalize().into_bytes().to_vec()
}

#[cfg(test)]
mod tests {
    use super::bearer_token_matches;

    #[test]
    fn bearer_token_comparison_accepts_exact_match() {
        assert!(bearer_token_matches(
            "token-1234567890ab",
            "token-1234567890ab"
        ));
    }

    #[test]
    fn bearer_token_comparison_rejects_same_length_mismatch() {
        assert!(!bearer_token_matches(
            "token-1234567890ac",
            "token-1234567890ab"
        ));
    }

    #[test]
    fn bearer_token_comparison_rejects_different_length_mismatch() {
        assert!(!bearer_token_matches("short", "token-1234567890ab"));
    }
}
