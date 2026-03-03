use axum::{
    http::StatusCode,
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
}

impl IntoResponse for AppError {
    /// Maps app errors into the standardized HTTP error response shape.
    ///
    /// Client responses remain opaque for internal failures, while detailed
    /// diagnostics are logged with an internal error identifier.
    fn into_response(self) -> Response {
        let (status, code, message) = match self {
            Self::BadRequest { code, message } => {
                (StatusCode::BAD_REQUEST, code, message.to_string())
            }
            Self::Unauthorized { code, message } => {
                (StatusCode::UNAUTHORIZED, code, message.to_string())
            }
            Self::Forbidden { code, message } => (StatusCode::FORBIDDEN, code, message.to_string()),
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
                )
            }
            Self::NotImplemented { code, message } => {
                (StatusCode::NOT_IMPLEMENTED, code, message.to_string())
            }
        };

        (
            status,
            Json(ErrorResponse {
                code: code.to_string(),
                message,
                details: json!({}),
            }),
        )
            .into_response()
    }
}
