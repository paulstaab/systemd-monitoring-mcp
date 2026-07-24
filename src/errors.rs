use axum::{
    http::{header, HeaderValue, StatusCode},
    response::{IntoResponse, Response},
    Json,
};
use serde::Serialize;
use serde_json::json;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum AppError {
    #[error("bad request: {message}")]
    BadRequest {
        code: &'static str,
        message: &'static str,
    },
    #[error("bad request: {message}")]
    BadRequestWithDetails {
        code: &'static str,
        message: &'static str,
        details: serde_json::Value,
    },
    #[error("unauthorized: {message}")]
    Unauthorized {
        code: &'static str,
        message: &'static str,
    },
    #[error("forbidden: {message}")]
    Forbidden {
        code: &'static str,
        message: &'static str,
    },
    #[error("internal error")]
    Internal { code: &'static str, message: String },
    #[error("not implemented: {message}")]
    NotImplemented {
        code: &'static str,
        message: &'static str,
    },
    #[error("rate limit exceeded")]
    TooManyRequests { retry_after_seconds: u64 },
}

#[derive(Debug, Serialize)]
pub struct ErrorResponse {
    pub code: String,
    pub message: String,
    pub details: serde_json::Value,
}

impl AppError {
    /// Creates a stable bad-request error for validation failures.
    pub fn bad_request(code: &'static str, message: &'static str) -> Self {
        Self::BadRequest { code, message }
    }

    /// Creates a validation error carrying stable structured details.
    pub fn bad_request_with_details(
        code: &'static str,
        message: &'static str,
        details: serde_json::Value,
    ) -> Self {
        Self::BadRequestWithDetails {
            code,
            message,
            details,
        }
    }

    /// Creates an unauthorized error used by auth and access checks.
    pub fn unauthorized(code: &'static str, message: &'static str) -> Self {
        Self::Unauthorized { code, message }
    }

    /// Creates a forbidden error for authenticated-but-disallowed operations.
    pub fn forbidden(code: &'static str, message: &'static str) -> Self {
        Self::Forbidden { code, message }
    }

    /// Creates an internal error preserving operator diagnostics server-side.
    pub fn internal(message: impl Into<String>) -> Self {
        Self::Internal {
            code: "internal_error",
            message: message.into(),
        }
    }

    /// Creates a not-implemented error for explicitly unsupported behavior.
    pub fn not_implemented(code: &'static str, message: &'static str) -> Self {
        Self::NotImplemented { code, message }
    }

    /// Creates a global-admission rejection with a whole-second retry delay.
    ///
    /// The delay is clamped to at least one second and emitted in the
    /// `Retry-After` header by the HTTP response conversion.
    pub fn too_many_requests(retry_after_seconds: u64) -> Self {
        Self::TooManyRequests {
            retry_after_seconds: retry_after_seconds.max(1),
        }
    }
}

impl IntoResponse for AppError {
    /// Maps app errors into the standardized HTTP error response shape.
    ///
    /// Client responses remain opaque for internal failures, while detailed
    /// diagnostics are logged with an internal error identifier. Rate-limit
    /// failures add a positive whole-second `Retry-After` header.
    fn into_response(self) -> Response {
        let (status, code, message, details, retry_after_seconds) = match self {
            Self::BadRequest { code, message } => (
                StatusCode::BAD_REQUEST,
                code,
                message.to_string(),
                json!({}),
                None,
            ),
            Self::BadRequestWithDetails {
                code,
                message,
                details,
            } => (
                StatusCode::BAD_REQUEST,
                code,
                message.to_string(),
                details,
                None,
            ),
            Self::Unauthorized { code, message } => (
                StatusCode::UNAUTHORIZED,
                code,
                message.to_string(),
                json!({}),
                None,
            ),
            Self::Forbidden { code, message } => (
                StatusCode::FORBIDDEN,
                code,
                message.to_string(),
                json!({}),
                None,
            ),
            Self::Internal { code, message } => {
                // Log internal diagnostics for operators while keeping HTTP responses opaque.
                let error_id = {
                    use std::collections::hash_map::DefaultHasher;
                    use std::hash::{Hash, Hasher};
                    let mut hasher = DefaultHasher::new();
                    message.hash(&mut hasher);
                    format!("{:016x}", hasher.finish())
                };
                tracing::error!(
                    error_id = %error_id,
                    detail = %message,
                    "request failed with internal error"
                );
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    code,
                    "internal server error".to_string(),
                    json!({}),
                    None,
                )
            }
            Self::NotImplemented { code, message } => (
                StatusCode::NOT_IMPLEMENTED,
                code,
                message.to_string(),
                json!({}),
                None,
            ),
            Self::TooManyRequests {
                retry_after_seconds,
            } => (
                StatusCode::TOO_MANY_REQUESTS,
                "rate_limit_exceeded",
                "rate limit exceeded".to_string(),
                json!({}),
                Some(retry_after_seconds.max(1)),
            ),
        };

        let mut response = (
            status,
            Json(ErrorResponse {
                code: code.to_string(),
                message,
                details,
            }),
        )
            .into_response();
        if let Some(retry_after_seconds) = retry_after_seconds {
            let value = HeaderValue::from_str(&retry_after_seconds.to_string())
                .expect("u64 Retry-After value must be a valid header");
            response.headers_mut().insert(header::RETRY_AFTER, value);
        }
        response
    }
}
