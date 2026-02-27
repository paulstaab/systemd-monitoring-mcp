use axum::{
    extract::connect_info::ConnectInfo,
    extract::{Request, State},
    http::header,
    middleware::Next,
    response::Response,
};

use crate::{errors::AppError, AppState};

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

    if provided_token != state.api_token.as_ref() {
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
        let connect_info = request
            .extensions()
            .get::<ConnectInfo<std::net::SocketAddr>>()
            .ok_or_else(|| {
                AppError::forbidden(
                    "ip_restricted",
                    "request source IP is unavailable for allowlist validation",
                )
            })?;

        if !allowed_cidr.contains(&connect_info.0.ip()) {
            return Err(AppError::forbidden(
                "ip_restricted",
                "request source IP is not allowed",
            ));
        }
    }

    Ok(next.run(request).await)
}

fn parse_bearer_token(value: &str) -> Option<&str> {
    value
        .strip_prefix("Bearer ")
        .filter(|token| !token.is_empty())
}

#[cfg(test)]
mod tests {
    use super::parse_bearer_token;

    #[test]
    fn parses_bearer_token() {
        assert_eq!(parse_bearer_token("Bearer token"), Some("token"));
        assert_eq!(parse_bearer_token("Basic token"), None);
    }
}
