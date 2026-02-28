//! Axum HTTP handlers for the web server
//!
//! Provides the primary Model Context Protocol endpoint, and general metadata endpoints.

use axum::{
    body::Bytes,
    extract::State,
    http::StatusCode,
    response::{IntoResponse, Response},
    Json,
};
use serde::Serialize;
use serde_json::Value;

use crate::mcp::rpc::json_rpc_error;
use crate::mcp::server::handle_json_rpc_value;
use crate::AppState;

#[derive(Debug, Serialize)]
pub struct HealthResponse {
    pub status: &'static str,
}

#[derive(Debug, Serialize)]
pub struct DiscoveryResponse {
    pub name: &'static str,
    pub version: &'static str,
    pub mcp_endpoint: &'static str,
}

pub async fn health() -> Json<HealthResponse> {
    Json(HealthResponse { status: "ok" })
}

pub async fn discovery() -> Json<DiscoveryResponse> {
    Json(DiscoveryResponse {
        name: env!("CARGO_PKG_NAME"),
        version: env!("CARGO_PKG_VERSION"),
        mcp_endpoint: "/mcp",
    })
}

pub async fn mcp_endpoint(State(state): State<AppState>, body: Bytes) -> Response {
    let payload: Value = match serde_json::from_slice(&body) {
        Ok(value) => value,
        Err(_) => {
            return (
                StatusCode::OK,
                Json(json_rpc_error(None, -32700, "Parse error")),
            )
                .into_response()
        }
    };

    if let Some(batch) = payload.as_array() {
        if batch.is_empty() {
            return (
                StatusCode::OK,
                Json(vec![json_rpc_error(None, -32600, "Invalid Request")]),
            )
                .into_response();
        }

        let mut responses = Vec::new();
        for item in batch {
            if let Some(response) = handle_json_rpc_value(&state, item.clone()).await {
                responses.push(response);
            }
        }

        if responses.is_empty() {
            return StatusCode::NO_CONTENT.into_response();
        }

        return (StatusCode::OK, Json(Value::Array(responses))).into_response();
    }

    match handle_json_rpc_value(&state, payload).await {
        Some(response) => (StatusCode::OK, Json(response)).into_response(),
        None => StatusCode::NO_CONTENT.into_response(),
    }
}
