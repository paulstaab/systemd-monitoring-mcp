use axum::{
    extract::{Request, State},
    http::header,
    http::HeaderValue,
    middleware::Next,
    response::Response,
};
use axum_extra::headers::{authorization::Bearer, Authorization, Header};

use crate::{errors::AppError, AppState};

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

    if auth.token() != state.api_token.as_ref() {
        return Err(AppError::unauthorized(
            "invalid_token",
            "invalid bearer token",
        ));
    }

    Ok(next.run(request).await)
}

fn decode_bearer_authorization(value: &HeaderValue) -> Option<Authorization<Bearer>> {
    let mut header_values = std::iter::once(value);
    Authorization::<Bearer>::decode(&mut header_values).ok()
}
