//! The central Model Context Protocol engine
//!
//! Provides the primary MCP JSON-RPC decoding, method execution routing, capabilities
//! negotiation (`initialize`), and tool/resource integrations routing mapping.

use chrono::NaiveDate;
use rust_mcp_sdk::schema::{JsonrpcMessage, JsonrpcRequest, ProtocolVersion};
use serde_json::{Value, json};
use tracing::info;

use crate::domain::{
    resources::{build_resources_list, handle_resources_read},
    tools::{build_tools_list, handle_tools_call},
};
use crate::mcp::rpc::{
    app_error_to_json_rpc, is_json_rpc_error, json_rpc_invalid_params, json_rpc_invalid_request,
    json_rpc_method_not_found, json_rpc_result, request_id_to_value,
};
use crate::{AppState, errors::AppError};

pub const MIN_SUPPORTED_PROTOCOL_VERSION: &str = "2024-11-05";
pub const FALLBACK_PROTOCOL_VERSION: ProtocolVersion = ProtocolVersion::V2025_03_26;

/// Handles a single JSON value as an MCP JSON-RPC message.
///
/// Supports request and notification flows and returns `None` for notification-only
/// handling where no response body should be sent.
pub async fn handle_json_rpc_value(state: &AppState, payload: Value) -> Option<Value> {
    if !payload.is_object() {
        return Some(json_rpc_invalid_request(None));
    }

    let request_id = payload.get("id").cloned();
    if request_id
        .as_ref()
        .is_some_and(|id| !is_valid_request_id(id))
    {
        return Some(json_rpc_invalid_request(None));
    }

    let parsed: JsonrpcMessage = match serde_json::from_value(payload) {
        Ok(message) => message,
        Err(_) => return Some(json_rpc_invalid_request(request_id)),
    };

    match parsed {
        JsonrpcMessage::Request(request) => {
            if let Err(error_response) = validate_request_shape(&request) {
                return Some(error_response);
            }

            let request_id = request_id_to_value(request.id);
            if request.method.trim().is_empty() {
                return Some(json_rpc_invalid_request(Some(request_id)));
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
            Some(json_rpc_invalid_request(request_id))
        }
    }
}

/// Returns whether a present JSON-RPC request ID has a supported scalar type.
///
/// MCP request IDs are strings or signed integers. Rejecting every other JSON
/// type before untagged message decoding prevents malformed requests from being
/// misclassified and executed as notifications.
fn is_valid_request_id(id: &Value) -> bool {
    id.is_string() || id.as_i64().is_some()
}

/// Validates method-specific request envelope shape before dispatch.
///
/// Returns JSON-RPC `-32602` when method params do not satisfy expected schema.
pub fn validate_request_shape(request: &JsonrpcRequest) -> Result<(), Value> {
    let request_id = Some(request_id_to_value(request.id.clone()));

    let valid = match request.method.as_str() {
        "tools/call" => validate_tool_call_params(request),
        "resources/read" => validate_resource_read_params(request),
        "tools/list" | "resources/list" | "ping" => true,
        "initialize" => validate_initialize_request_params(request),
        _ => true,
    };

    if valid {
        Ok(())
    } else {
        Err(json_rpc_invalid_params(request_id))
    }
}

/// Validates the stable tools/call parameter shape.
///
/// A tool name is required and arguments, when supplied, must be an object.
/// Tool-specific argument validation remains in the selected handler.
fn validate_tool_call_params(request: &JsonrpcRequest) -> bool {
    let Some(params) = request.params.as_ref() else {
        return false;
    };

    params.get("name").is_some_and(Value::is_string)
        && params.get("arguments").is_none_or(Value::is_object)
}

/// Validates the stable resources/read parameter shape.
///
/// Resource routing only requires a string URI; unknown but well-formed URIs
/// are dispatched so the handler can return its stable resource-not-found error.
fn validate_resource_read_params(request: &JsonrpcRequest) -> bool {
    request
        .params
        .as_ref()
        .and_then(|params| params.get("uri"))
        .is_some_and(Value::is_string)
}

/// Validates the stable initialize fields supported by this server.
///
/// The SDK's default schema currently targets a newer draft that removed the
/// initialize request, so validation is kept local to the protocol versions
/// advertised by this server. Extra capability and client-info fields remain
/// allowed, while the three required fields must have their MCP-defined types.
fn validate_initialize_request_params(request: &JsonrpcRequest) -> bool {
    let Some(params) = request.params.as_ref() else {
        return false;
    };
    let Some(client_info) = params.get("clientInfo").and_then(Value::as_object) else {
        return false;
    };

    params.get("protocolVersion").is_some_and(Value::is_string)
        && params.get("capabilities").is_some_and(Value::is_object)
        && client_info.get("name").is_some_and(Value::is_string)
        && client_info.get("version").is_some_and(Value::is_string)
}

/// Executes a parsed JSON-RPC request method and returns a response payload.
///
/// Also emits MCP audit logs with redacted parameter content.
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

            let initialize_result = json!({
                "protocolVersion": protocol_version.to_string(),
                "capabilities": {
                    "tools": { "listChanged": false },
                    "resources": { "subscribe": false, "listChanged": false }
                },
                "serverInfo": {
                    "name": env!("CARGO_PKG_NAME"),
                    "version": env!("CARGO_PKG_VERSION")
                }
            });

            json_rpc_result(id, initialize_result)
        }
        "ping" => json_rpc_result(id, json!({})),
        "tools/list" => json_rpc_result(id, json!({ "tools": build_tools_list() })),
        "tools/call" => handle_tools_call(state, id, params).await,
        "resources/list" => json_rpc_result(id, json!({ "resources": build_resources_list() })),
        "resources/read" => handle_resources_read(state, id, params).await,
        _ => json_rpc_method_not_found(id),
    };

    info!(
        method = %method,
        params = %audit_params,
        outcome = if is_json_rpc_error(&response) { "failure" } else { "success" },
        "mcp action audited"
    );

    response
}

/// Negotiates protocol version from initialize request params.
///
/// Accepts known versions directly and falls back to the configured fallback
/// for unknown-but-newer date versions above the minimum supported version.
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

    if let Ok(protocol_version) = ProtocolVersion::try_from(offered_version) {
        return Ok(protocol_version);
    }

    let offered_date = parse_protocol_version_date(offered_version).ok_or_else(|| {
        AppError::bad_request(
            "unsupported_protocol_version",
            "unsupported initialize protocolVersion",
        )
    })?;

    let min_supported_date = parse_protocol_version_date(MIN_SUPPORTED_PROTOCOL_VERSION)
        .expect("MIN_SUPPORTED_PROTOCOL_VERSION must be valid YYYY-MM-DD");

    if offered_date >= min_supported_date {
        return Ok(FALLBACK_PROTOCOL_VERSION);
    }

    Err(AppError::bad_request(
        "unsupported_protocol_version",
        "unsupported initialize protocolVersion",
    ))
}

/// Parses protocol version strings in `YYYY-MM-DD` format.
///
/// Returns `None` for malformed or invalid calendar dates.
fn parse_protocol_version_date(version: &str) -> Option<NaiveDate> {
    if version.len() != 10 {
        return None;
    }

    let mut parts = version.split('-');
    let (Some(year), Some(month), Some(day), None) =
        (parts.next(), parts.next(), parts.next(), parts.next())
    else {
        return None;
    };

    if year.len() != 4
        || month.len() != 2
        || day.len() != 2
        || !year.chars().all(|character| character.is_ascii_digit())
        || !month.chars().all(|character| character.is_ascii_digit())
        || !day.chars().all(|character| character.is_ascii_digit())
    {
        return None;
    }

    let year = year.parse::<i32>().ok()?;
    let month = month.parse::<u32>().ok()?;
    let day = day.parse::<u32>().ok()?;

    NaiveDate::from_ymd_opt(year, month, day)
}

/// Redacts sensitive values in optional audit parameter payloads.
pub fn redact_audit_params(params: Option<&Value>) -> Value {
    params.map(redact_audit_value).unwrap_or(Value::Null)
}

/// Recursively redacts sensitive keys from JSON values for logging safety.
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

/// Returns whether a key should be treated as sensitive for audit redaction.
///
/// Matches exact credential terms and common credential-related substrings.
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
    use super::{
        FALLBACK_PROTOCOL_VERSION, MIN_SUPPORTED_PROTOCOL_VERSION, is_valid_request_id,
        negotiate_protocol_version, redact_audit_params,
    };
    use serde_json::json;

    /// Covers every JSON type accepted or rejected for a present request ID.
    #[test]
    fn validates_request_id_scalar_types() {
        assert!(is_valid_request_id(&json!(1)));
        assert!(is_valid_request_id(&json!("request-1")));
        for invalid in [json!(null), json!(1.5), json!([]), json!({})] {
            assert!(!is_valid_request_id(&invalid));
        }
    }

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
            "protocolVersion": MIN_SUPPORTED_PROTOCOL_VERSION
        });

        let version = negotiate_protocol_version(Some(&params)).expect("supported version");
        assert_eq!(version, rust_mcp_sdk::schema::ProtocolVersion::V2024_11_05);
    }

    #[test]
    fn negotiate_protocol_version_accepts_modern_supported_version() {
        let params = json!({
            "protocolVersion": "2025-03-26"
        });

        let version = negotiate_protocol_version(Some(&params)).expect("modern version");
        assert_eq!(version, rust_mcp_sdk::schema::ProtocolVersion::V2025_03_26);
    }

    #[test]
    fn negotiate_protocol_version_falls_back_for_newer_unknown_version() {
        let params = json!({
            "protocolVersion": "2026-01-01"
        });

        let version = negotiate_protocol_version(Some(&params)).expect("fallback version");
        assert_eq!(version, FALLBACK_PROTOCOL_VERSION);
    }

    #[test]
    fn negotiate_protocol_version_rejects_too_old_version() {
        let params = json!({
            "protocolVersion": "2024-01-01"
        });

        let error =
            negotiate_protocol_version(Some(&params)).expect_err("unsupported version must fail");
        assert!(error.to_string().contains("bad request"));
    }

    #[test]
    fn negotiate_protocol_version_rejects_malformed_version() {
        let params = json!({
            "protocolVersion": "2024-9-01"
        });

        let error =
            negotiate_protocol_version(Some(&params)).expect_err("malformed version must fail");
        assert!(error.to_string().contains("bad request"));
    }

    #[test]
    fn negotiate_protocol_version_rejects_non_date_version() {
        let params = json!({
            "protocolVersion": "future"
        });

        let error =
            negotiate_protocol_version(Some(&params)).expect_err("non-date version must fail");
        assert!(error.to_string().contains("bad request"));
    }
}
