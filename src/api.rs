use axum::{body::Bytes, extract::State, Json};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::{errors::AppError, systemd_client::UnitStatus, AppState};

#[derive(Debug, Serialize)]
pub struct HealthResponse {
    pub status: &'static str,
}

#[derive(Debug, Serialize)]
pub struct DiscoveryResponse {
    pub name: &'static str,
    pub version: &'static str,
    pub mcp_endpoint: &'static str,
    pub services_endpoint: &'static str,
}

#[derive(Debug, Deserialize)]
struct JsonRpcRequest {
    jsonrpc: String,
    method: String,
    #[serde(default)]
    id: Option<Value>,
}

pub async fn health() -> Json<HealthResponse> {
    Json(HealthResponse { status: "ok" })
}

pub async fn discovery() -> Json<DiscoveryResponse> {
    Json(DiscoveryResponse {
        name: env!("CARGO_PKG_NAME"),
        version: env!("CARGO_PKG_VERSION"),
        mcp_endpoint: "/mcp",
        services_endpoint: "/services",
    })
}

pub async fn mcp_endpoint(body: Bytes) -> Json<Value> {
    let parsed: JsonRpcRequest = match serde_json::from_slice(&body) {
        Ok(request) => request,
        Err(_) => return Json(json_rpc_error(None, -32700, "Parse error")),
    };

    if parsed.jsonrpc != "2.0" || parsed.method.trim().is_empty() {
        return Json(json_rpc_error(parsed.id, -32600, "Invalid Request"));
    }

    match parsed.method.as_str() {
        "initialize" => Json(json!({
            "jsonrpc": "2.0",
            "id": parsed.id,
            "result": {
                "protocolVersion": "2024-11-05",
                "serverInfo": {
                    "name": env!("CARGO_PKG_NAME"),
                    "version": env!("CARGO_PKG_VERSION")
                },
                "capabilities": {
                    "tools": {
                        "listChanged": false
                    },
                    "resources": {
                        "subscribe": false,
                        "listChanged": false
                    },
                    "prompts": {
                        "listChanged": false
                    }
                },
                "metadata": {
                    "restEndpoints": {
                        "services": "/services"
                    }
                }
            }
        })),
        "ping" => Json(json!({
            "jsonrpc": "2.0",
            "id": parsed.id,
            "result": {}
        })),
        _ => Json(json_rpc_error(parsed.id, -32601, "Method not found")),
    }
}

pub async fn list_services(
    State(state): State<AppState>,
) -> Result<Json<Vec<UnitStatus>>, AppError> {
    let units = state.unit_provider.list_service_units().await?;
    Ok(Json(units))
}

fn json_rpc_error(id: Option<Value>, code: i32, message: &str) -> Value {
    json!({
        "jsonrpc": "2.0",
        "id": id,
        "error": {
            "code": code,
            "message": message
        }
    })
}
