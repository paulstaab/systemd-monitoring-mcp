use axum::{
    extract::{Request, State},
    middleware::Next,
    response::Response,
};
use axum_extra::{
    headers::{authorization::Bearer, Authorization},
    TypedHeader,
};

use crate::{errors::AppError, AppState};

pub async fn require_bearer_token(
    State(state): State<AppState>,
    auth_header: Option<TypedHeader<Authorization<Bearer>>>,
    request: Request,
    next: Next,
) -> Result<Response, AppError> {
    let Some(TypedHeader(auth)) = auth_header else {
        return Err(AppError::unauthorized(
            "missing_token",
            "missing authorization header",
        ));
    };

    if auth.token() != state.api_token.as_ref() {
        return Err(AppError::unauthorized(
            "invalid_token",
            "invalid bearer token",
        ));
    }

    Ok(next.run(request).await)
}
