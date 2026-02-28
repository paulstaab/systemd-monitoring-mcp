//! The central Model Context Protocol engine
//!
//! Provides the primary MCP JSON-RPC decoding, method execution routing, capabilities
//! negotiation (`initialize`), and tool/resource integrations routing mapping.

use rust_mcp_sdk::schema::{
    CallToolRequest, Implementation, InitializeRequest, InitializeResult, JsonrpcMessage,
    JsonrpcRequest, ListResourcesRequest, ListResourcesResult, ListToolsRequest, ListToolsResult,
    PingRequest, ProtocolVersion, ReadResourceRequest, ServerCapabilities,
    ServerCapabilitiesResources, ServerCapabilitiesTools,
};
use serde_json::{json, Value};
use tracing::info;

use crate::domain::{
    resources::{build_resources_list, handle_resources_read},
    tools::{build_tools_list, handle_tools_call},
};
use crate::mcp::rpc::{
    app_error_to_json_rpc, is_json_rpc_error, json_rpc_error, json_rpc_result, request_id_to_value,
};
use crate::{errors::AppError, AppState};

pub const SUPPORTED_PROTOCOL_VERSION: &str = "2024-11-05";

pub async fn handle_json_rpc_value(state: &AppState, payload: Value) -> Option<Value> {
    if !payload.is_object() {
        return Some(json_rpc_error(None, -32600, "Invalid Request"));
    }

    let request_id = payload.get("id").cloned();
    let parsed: JsonrpcMessage = match serde_json::from_value(payload) {
        Ok(message) => message,
        Err(_) => return Some(json_rpc_error(request_id, -32600, "Invalid Request")),
    };

    match parsed {
        JsonrpcMessage::Request(request) => {
            if let Err(error_response) = validate_request_shape(&request) {
                return Some(error_response);
            }

            let request_id = request_id_to_value(request.id);
            if request.method.trim().is_empty() {
                return Some(json_rpc_error(Some(request_id), -32600, "Invalid Request"));
            }

            Some(
                handle_json_rpc_request(
                    state,
                    Some(request_id),
                    request.method,
                    request.params.map(Value::Object),
                )
                .await,
            )
        }
        JsonrpcMessage::Notification(notification) => {
            if notification.method.trim().is_empty() {
                return None;
            }

            let _ = handle_json_rpc_request(
                state,
                None,
                notification.method,
                notification.params.map(Value::Object),
            )
            .await;
            None
        }
        JsonrpcMessage::ResultResponse(_) | JsonrpcMessage::ErrorResponse(_) => {
            Some(json_rpc_error(request_id, -32600, "Invalid Request"))
        }
    }
}

pub fn validate_request_shape(request: &JsonrpcRequest) -> Result<(), Value> {
    let payload = serde_json::to_value(request).expect("jsonrpc request serialization");
    let request_id = Some(request_id_to_value(request.id.clone()));

    let valid = match request.method.as_str() {
        "tools/call" => serde_json::from_value::<CallToolRequest>(payload).is_ok(),
        "resources/read" => serde_json::from_value::<ReadResourceRequest>(payload).is_ok(),
        "tools/list" => serde_json::from_value::<ListToolsRequest>(payload).is_ok(),
        "resources/list" => serde_json::from_value::<ListResourcesRequest>(payload).is_ok(),
        "ping" => serde_json::from_value::<PingRequest>(payload).is_ok(),
        "initialize" => serde_json::from_value::<InitializeRequest>(payload).is_ok(),
        _ => true,
    };

    if valid {
        Ok(())
    } else {
        Err(json_rpc_error(request_id, -32602, "Invalid params"))
    }
}

pub async fn handle_json_rpc_request(
    state: &AppState,
    id: Option<Value>,
    method: String,
    params: Option<Value>,
) -> Value {
    let audit_params = redact_audit_params(params.as_ref());

    let response = match method.as_str() {
        "initialize" => {
            let protocol_version = match negotiate_protocol_version(params.as_ref()) {
                Ok(version) => version,
                Err(err) => return app_error_to_json_rpc(id, err),
            };

            let initialize_result = InitializeResult {
                server_info: Implementation {
                    name: env!("CARGO_PKG_NAME").to_string(),
                    version: env!("CARGO_PKG_VERSION").to_string(),
                    title: None,
                    description: None,
                    icons: vec![],
                    website_url: None,
                },
                capabilities: ServerCapabilities {
                    tools: Some(ServerCapabilitiesTools {
                        list_changed: Some(false),
                    }),
                    resources: Some(ServerCapabilitiesResources {
                        subscribe: Some(false),
                        list_changed: Some(false),
                    }),
                    prompts: None,
                    ..Default::default()
                },
                protocol_version: protocol_version.into(),
                instructions: None,
                meta: None,
            };

            json_rpc_result(
                id,
                serde_json::to_value(initialize_result).expect("initialize result serialization"),
            )
        }
        "ping" => json_rpc_result(id, json!({})),
        "tools/list" => json_rpc_result(
            id,
            serde_json::to_value(ListToolsResult {
                meta: None,
                next_cursor: None,
                tools: build_tools_list(),
            })
            .expect("tools list result serialization"),
        ),
        "tools/call" => handle_tools_call(state, id, params).await,
        "resources/list" => json_rpc_result(
            id,
            serde_json::to_value(ListResourcesResult {
                meta: None,
                next_cursor: None,
                resources: build_resources_list(),
            })
            .expect("resources list result serialization"),
        ),
        "resources/read" => handle_resources_read(state, id, params).await,
        _ => json_rpc_error(id, -32601, "Method not found"),
    };

    info!(
        method = %method,
        params = %audit_params,
        outcome = if is_json_rpc_error(&response) { "failure" } else { "success" },
        "mcp action audited"
    );

    response
}

pub fn negotiate_protocol_version(params: Option<&Value>) -> Result<ProtocolVersion, AppError> {
    let offered_version = params
        .and_then(Value::as_object)
        .and_then(|object| object.get("protocolVersion"))
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|version| !version.is_empty())
        .ok_or_else(|| {
            AppError::bad_request(
                "invalid_protocol_version",
                "initialize params.protocolVersion is required",
            )
        })?;

    if offered_version != SUPPORTED_PROTOCOL_VERSION {
        return Err(AppError::bad_request(
            "unsupported_protocol_version",
            "unsupported initialize protocolVersion",
        ));
    }

    Ok(ProtocolVersion::V2024_11_05)
}

pub fn redact_audit_params(params: Option<&Value>) -> Value {
    params.map(redact_audit_value).unwrap_or(Value::Null)
}

pub fn redact_audit_value(value: &Value) -> Value {
    match value {
        Value::Object(map) => Value::Object(
            map.iter()
                .map(|(key, item)| {
                    if is_sensitive_key(key) {
                        (key.clone(), Value::String("[REDACTED]".to_string()))
                    } else {
                        (key.clone(), redact_audit_value(item))
                    }
                })
                .collect(),
        ),
        Value::Array(items) => Value::Array(items.iter().map(redact_audit_value).collect()),
        _ => value.clone(),
    }
}

pub fn is_sensitive_key(key: &str) -> bool {
    let normalized = key.trim().to_ascii_lowercase();
    matches!(
        normalized.as_str(),
        "token"
            | "api_token"
            | "access_token"
            | "refresh_token"
            | "authorization"
            | "bearer"
            | "password"
            | "secret"
            | "credentials"
            | "credential"
            | "api_key"
            | "apikey"
    ) || normalized.contains("token")
        || normalized.contains("secret")
        || normalized.contains("password")
        || normalized.contains("credential")
}

#[cfg(test)]
mod tests {
    use super::{negotiate_protocol_version, redact_audit_params, SUPPORTED_PROTOCOL_VERSION};
    use serde_json::json;

    #[test]
    fn redacts_sensitive_fields_in_audit_params() {
        let params = json!({
            "name": "list_logs",
            "arguments": {
                "unit": "sshd.service",
                "token": "should-not-appear",
                "api_key": "should-not-appear",
                "nested": {
                    "secret": "should-not-appear"
                }
            }
        });

        let redacted = redact_audit_params(Some(&params));

        assert_eq!(redacted["name"], json!("list_logs"));
        assert_eq!(redacted["arguments"]["unit"], json!("sshd.service"));
        assert_eq!(redacted["arguments"]["token"], json!("[REDACTED]"));
        assert_eq!(redacted["arguments"]["api_key"], json!("[REDACTED]"));
        assert_eq!(
            redacted["arguments"]["nested"]["secret"],
            json!("[REDACTED]")
        );
    }

    #[test]
    fn negotiate_protocol_version_accepts_supported_version() {
        let params = json!({
            "protocolVersion": SUPPORTED_PROTOCOL_VERSION
        });

        let version = negotiate_protocol_version(Some(&params)).expect("supported version");
        assert_eq!(version, rust_mcp_sdk::schema::ProtocolVersion::V2024_11_05);
    }

    #[test]
    fn negotiate_protocol_version_rejects_unsupported_version() {
        let params = json!({
            "protocolVersion": "2026-01-01"
        });

        let error =
            negotiate_protocol_version(Some(&params)).expect_err("unsupported version must fail");
        assert!(error.to_string().contains("bad request"));
    }
}
