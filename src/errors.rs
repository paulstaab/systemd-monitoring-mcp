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
    #[error("unauthorized: {message}")]
    Unauthorized {
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
    pub fn unauthorized(code: &'static str, message: &'static str) -> Self {
        Self::Unauthorized { code, message }
    }

    pub fn internal(message: impl Into<String>) -> Self {
        Self::Internal {
            code: "internal_error",
            message: message.into(),
        }
    }

    pub fn not_implemented(code: &'static str, message: &'static str) -> Self {
        Self::NotImplemented { code, message }
    }
}

impl IntoResponse for AppError {
    fn into_response(self) -> Response {
        let (status, code, message) = match self {
            Self::Unauthorized { code, message } => {
                (StatusCode::UNAUTHORIZED, code, message.to_string())
            }
            Self::Internal { code, message } => {
                tracing::error!(error = %message, "request failed with internal error");
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
