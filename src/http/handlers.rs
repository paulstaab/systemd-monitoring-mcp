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
use serde_json::{json, Value};

use crate::mcp::rpc::{json_rpc_invalid_request, json_rpc_parse_error};
use crate::mcp::server::handle_json_rpc_value;
use crate::systemd_client::UnitScope;
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

#[derive(Debug, Serialize)]
pub struct SystemdStatusResponse {
    pub scope: &'static str,
    pub status: String,
}

/// Lightweight health handler for infrastructure liveness checks.
pub async fn health() -> Json<HealthResponse> {
    Json(HealthResponse { status: "ok" })
}

/// Public MCP discovery metadata handler.
///
/// Exposes only package identity and MCP endpoint path.
pub async fn discovery() -> Json<DiscoveryResponse> {
    Json(DiscoveryResponse {
        name: env!("CARGO_PKG_NAME"),
        version: env!("CARGO_PKG_VERSION"),
        mcp_endpoint: "/mcp",
    })
}

/// Reports the system systemd manager state for uptime-style checks.
///
/// Returns `200 OK` only when the manager reports `running`; `degraded` and any
/// other non-running state return `503` with structured HTTP error details so
/// monitors can fail closed while still showing the observed manager state.
pub async fn systemd_system_status(State(state): State<AppState>) -> Response {
    systemd_status(&state, UnitScope::System).await
}

/// Reports the user systemd manager state for uptime-style checks.
///
/// Returns the same response contract as the system-scope endpoint but queries
/// the user manager via the session D-Bus connection. A missing user session is
/// treated as an internal dependency failure by the provider.
pub async fn systemd_user_status(State(state): State<AppState>) -> Response {
    systemd_status(&state, UnitScope::User).await
}

/// Builds the shared systemd status response for one concrete manager scope.
///
/// `running` maps to a successful JSON body containing `scope` and `status`.
/// Any other value, especially `degraded`, maps to HTTP `503` using the
/// repository-wide structured error response shape with the manager state in
/// `details`.
async fn systemd_status(state: &AppState, scope: UnitScope) -> Response {
    let scope_name = scope.as_str();
    let status = match state.unit_provider.system_state(scope).await {
        Ok(status) => status,
        Err(err) => return err.into_response(),
    };

    if status == "running" {
        return (
            StatusCode::OK,
            Json(SystemdStatusResponse {
                scope: scope_name,
                status,
            }),
        )
            .into_response();
    }

    (
        StatusCode::SERVICE_UNAVAILABLE,
        Json(json!({
            "code": "systemd_not_running",
            "message": "systemd manager is not running",
            "details": {
                "scope": scope_name,
                "status": status,
            },
        })),
    )
        .into_response()
}

/// Main MCP transport endpoint handling single and batch JSON-RPC payloads.
///
/// Returns JSON-RPC parse errors for invalid JSON, no-content for notification-only
/// requests, and HTTP 200 for valid JSON-RPC responses.
pub async fn mcp_endpoint(State(state): State<AppState>, body: Bytes) -> Response {
    let payload: Value = match serde_json::from_slice(&body) {
        Ok(value) => value,
        Err(_) => return (StatusCode::OK, Json(json_rpc_parse_error(None))).into_response(),
    };

    if let Some(batch) = payload.as_array() {
        if batch.is_empty() {
            return (StatusCode::OK, Json(vec![json_rpc_invalid_request(None)])).into_response();
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
