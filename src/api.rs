use axum::{
    body::Bytes,
    extract::State,
    http::StatusCode,
    response::{IntoResponse, Response},
    Json,
};
use chrono::{DateTime, Duration, Utc};
use rust_mcp_sdk::{
    macros,
    schema::{
        CallToolRequest, CallToolRequestParams, CallToolResult, ContentBlock, Implementation,
        InitializeRequest, InitializeResult, JsonrpcErrorResponse, JsonrpcMessage, JsonrpcRequest,
        JsonrpcResultResponse, ListResourcesRequest, ListResourcesResult, ListToolsRequest,
        ListToolsResult, PingRequest, ProtocolVersion, ReadResourceContent, ReadResourceRequest,
        ReadResourceRequestParams, ReadResourceResult, RequestId, Resource, Result as McpResult,
        RpcError, ServerCapabilities, ServerCapabilitiesResources, ServerCapabilitiesTools,
        TextContent, TextResourceContents, Tool,
    },
};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use tracing::info;

use crate::{
    errors::AppError,
    systemd_client::{LogQuery, UnitStatus},
    AppState,
};

const MAX_LOG_LIMIT: usize = 1_000;
const DEFAULT_LOG_LIMIT: usize = 100;
const SUPPORTED_PROTOCOL_VERSION: &str = "2024-11-05";
const VALID_SERVICE_STATES: [&str; 6] = [
    "active",
    "inactive",
    "failed",
    "activating",
    "deactivating",
    "reloading",
];

#[derive(Debug, Deserialize)]
struct ServicesQueryParams {
    state: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct LogsQueryParams {
    priority: Option<String>,
    unit: Option<String>,
    start_utc: Option<String>,
    end_utc: Option<String>,
    limit: Option<usize>,
}

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

const SERVICES_RESOURCE_URI: &str = "resource://services/snapshot";
const FAILED_SERVICES_RESOURCE_URI: &str = "resource://services/failed";
const LOGS_RESOURCE_URI: &str = "resource://logs/recent";

#[macros::mcp_tool(
    name = "list_services",
    description = "List systemd service units and current state"
)]
#[derive(Debug, Deserialize, Serialize, macros::JsonSchema)]
struct ListServicesTool {
    state: Option<String>,
}

#[macros::mcp_tool(
    name = "list_logs",
    description = "List journald logs with filters and bounds"
)]
#[derive(Debug, Deserialize, Serialize, macros::JsonSchema)]
struct ListLogsTool {
    priority: Option<String>,
    unit: Option<String>,
    start_utc: String,
    end_utc: String,
    limit: Option<u32>,
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

async fn handle_json_rpc_value(state: &AppState, payload: Value) -> Option<Value> {
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

fn validate_request_shape(request: &JsonrpcRequest) -> Result<(), Value> {
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

async fn handle_json_rpc_request(
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
                resources: vec![
                    Resource {
                        annotations: None,
                        description: Some("Current systemd service statuses".to_string()),
                        icons: vec![],
                        meta: None,
                        mime_type: Some("application/json".to_string()),
                        name: "Service Snapshot".to_string(),
                        size: None,
                        title: None,
                        uri: SERVICES_RESOURCE_URI.to_string(),
                    },
                    Resource {
                        annotations: None,
                        description: Some("Current failed systemd service statuses".to_string()),
                        icons: vec![],
                        meta: None,
                        mime_type: Some("application/json".to_string()),
                        name: "Failed Service Snapshot".to_string(),
                        size: None,
                        title: None,
                        uri: FAILED_SERVICES_RESOURCE_URI.to_string(),
                    },
                    Resource {
                        annotations: None,
                        description: Some("Recent journald logs for the last hour".to_string()),
                        icons: vec![],
                        meta: None,
                        mime_type: Some("application/json".to_string()),
                        name: "Recent Logs Snapshot".to_string(),
                        size: None,
                        title: None,
                        uri: LOGS_RESOURCE_URI.to_string(),
                    },
                ],
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

async fn handle_tools_call(state: &AppState, id: Option<Value>, params: Option<Value>) -> Value {
    let Some(raw_params) = params else {
        return json_rpc_error(id, -32602, "Invalid params");
    };

    let tool_call: CallToolRequestParams = match serde_json::from_value(raw_params) {
        Ok(value) => value,
        Err(_) => return json_rpc_error(id, -32602, "Invalid params"),
    };

    match tool_call.name.as_str() {
        "list_services" => {
            let query_params: ServicesQueryParams =
                match serde_json::from_value(json!(tool_call.arguments.unwrap_or_default())) {
                    Ok(value) => value,
                    Err(_) => return json_rpc_error(id, -32602, "Invalid params"),
                };

            let state_filter = match normalize_service_state(query_params.state) {
                Ok(value) => value,
                Err(err) => return app_error_to_json_rpc(id, err),
            };

            match state.unit_provider.list_service_units().await {
                Ok(services) => {
                    let services = filter_services_by_state(services, state_filter.as_deref());
                    json_rpc_result(
                        id,
                        serde_json::to_value(CallToolResult {
                            content: vec![ContentBlock::from(TextContent::new(
                                format!("Returned {} services", services.len()),
                                None,
                                None,
                            ))],
                            is_error: None,
                            meta: None,
                            structured_content: Some(serde_json::Map::from_iter([(
                                "services".to_string(),
                                json!(services),
                            )])),
                        })
                        .expect("list_services tool result serialization"),
                    )
                }
                Err(err) => app_error_to_json_rpc(id, err),
            }
        }
        "list_logs" => {
            let query_params: LogsQueryParams =
                match serde_json::from_value(json!(tool_call.arguments.unwrap_or_default())) {
                    Ok(value) => value,
                    Err(_) => return json_rpc_error(id, -32602, "Invalid params"),
                };

            let query = match build_log_query(query_params) {
                Ok(query) => query,
                Err(err) => return app_error_to_json_rpc(id, err),
            };

            match state.unit_provider.list_journal_logs(&query).await {
                Ok(logs) => json_rpc_result(
                    id,
                    serde_json::to_value(CallToolResult {
                        content: vec![ContentBlock::from(TextContent::new(
                            format!("Returned {} log entries", logs.len()),
                            None,
                            None,
                        ))],
                        is_error: None,
                        meta: None,
                        structured_content: Some(serde_json::Map::from_iter([(
                            "logs".to_string(),
                            json!(logs),
                        )])),
                    })
                    .expect("list_logs tool result serialization"),
                ),
                Err(err) => app_error_to_json_rpc(id, err),
            }
        }
        _ => json_rpc_error_with_data(
            id,
            -32601,
            "Method not found",
            Some(json!({
                "code": "tool_not_found",
                "message": "unknown tool name",
                "details": {
                    "name": tool_call.name,
                },
            })),
        ),
    }
}

async fn handle_resources_read(
    state: &AppState,
    id: Option<Value>,
    params: Option<Value>,
) -> Value {
    let Some(raw_params) = params else {
        return json_rpc_error(id, -32602, "Invalid params");
    };

    let resource_read: ReadResourceRequestParams = match serde_json::from_value(raw_params) {
        Ok(value) => value,
        Err(_) => return json_rpc_error(id, -32602, "Invalid params"),
    };

    match resource_read.uri.as_str() {
        SERVICES_RESOURCE_URI => match state.unit_provider.list_service_units().await {
            Ok(services) => {
                let structured_content = json!({ "services": services });
                let result = serde_json::to_value(ReadResourceResult {
                    contents: vec![ReadResourceContent::from(TextResourceContents {
                        meta: None,
                        mime_type: Some("application/json".to_string()),
                        text: structured_content.to_string(),
                        uri: SERVICES_RESOURCE_URI.to_string(),
                    })],
                    meta: None,
                })
                .expect("read services result serialization");

                json_rpc_result(id, result)
            }
            Err(err) => app_error_to_json_rpc(id, err),
        },
        FAILED_SERVICES_RESOURCE_URI => match state.unit_provider.list_service_units().await {
            Ok(services) => {
                let services = filter_services_by_state(services, Some("failed"));
                let structured_content = json!({ "services": services });
                let result = serde_json::to_value(ReadResourceResult {
                    contents: vec![ReadResourceContent::from(TextResourceContents {
                        meta: None,
                        mime_type: Some("application/json".to_string()),
                        text: structured_content.to_string(),
                        uri: FAILED_SERVICES_RESOURCE_URI.to_string(),
                    })],
                    meta: None,
                })
                .expect("read failed services result serialization");

                json_rpc_result(id, result)
            }
            Err(err) => app_error_to_json_rpc(id, err),
        },
        LOGS_RESOURCE_URI => {
            let end_utc = Utc::now();
            let start_utc = end_utc - Duration::hours(1);
            let query = LogQuery {
                priority: None,
                unit: None,
                start_utc: Some(start_utc),
                end_utc: Some(end_utc),
                limit: DEFAULT_LOG_LIMIT,
            };

            match state.unit_provider.list_journal_logs(&query).await {
                Ok(logs) => {
                    let structured_content = json!({ "logs": logs });
                    let result = serde_json::to_value(ReadResourceResult {
                        contents: vec![ReadResourceContent::from(TextResourceContents {
                            meta: None,
                            mime_type: Some("application/json".to_string()),
                            text: structured_content.to_string(),
                            uri: LOGS_RESOURCE_URI.to_string(),
                        })],
                        meta: None,
                    })
                    .expect("read logs result serialization");

                    json_rpc_result(id, result)
                }
                Err(err) => app_error_to_json_rpc(id, err),
            }
        }
        _ => json_rpc_error_with_data(
            id,
            -32601,
            "Method not found",
            Some(json!({
                "code": "resource_not_found",
                "message": "unknown resource uri",
                "details": {
                    "uri": resource_read.uri,
                },
            })),
        ),
    }
}

fn negotiate_protocol_version(params: Option<&Value>) -> Result<ProtocolVersion, AppError> {
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

fn is_json_rpc_error(value: &Value) -> bool {
    value.get("error").is_some()
}

fn redact_audit_params(params: Option<&Value>) -> Value {
    params.map(redact_audit_value).unwrap_or(Value::Null)
}

fn redact_audit_value(value: &Value) -> Value {
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

fn is_sensitive_key(key: &str) -> bool {
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

fn build_tools_list() -> Vec<Tool> {
    vec![ListServicesTool::tool(), ListLogsTool::tool()]
}

fn app_error_to_json_rpc(id: Option<Value>, err: AppError) -> Value {
    match err {
        AppError::BadRequest { code, message } => json_rpc_error_with_data(
            id,
            -32602,
            "Invalid params",
            Some(json!({
                "code": code,
                "message": message,
                "details": {}
            })),
        ),
        AppError::Unauthorized { code, message } | AppError::Forbidden { code, message } => {
            json_rpc_error_with_data(
                id,
                -32001,
                "Unauthorized",
                Some(json!({
                    "code": code,
                    "message": message,
                    "details": {}
                })),
            )
        }
        AppError::Internal { .. } | AppError::NotImplemented { .. } => {
            json_rpc_error(id, -32603, "Internal error")
        }
    }
}

fn build_log_query(params: LogsQueryParams) -> Result<LogQuery, AppError> {
    let start_utc = parse_utc(&params.start_utc)?;
    let end_utc = parse_utc(&params.end_utc)?;

    if start_utc.is_none() || end_utc.is_none() {
        return Err(AppError::bad_request(
            "missing_time_range",
            "start_utc and end_utc are required",
        ));
    }

    if let (Some(start), Some(end)) = (start_utc, end_utc) {
        if start > end {
            return Err(AppError::bad_request(
                "invalid_time_range",
                "start_utc must be less than or equal to end_utc",
            ));
        }
    }

    let limit = params.limit.unwrap_or(DEFAULT_LOG_LIMIT);
    if limit == 0 || limit > MAX_LOG_LIMIT {
        return Err(AppError::bad_request(
            "invalid_limit",
            "limit must be between 1 and 1000",
        ));
    }

    Ok(LogQuery {
        priority: normalize_priority(params.priority)?,
        unit: normalize_unit(params.unit)?,
        start_utc,
        end_utc,
        limit,
    })
}

fn parse_utc(value: &Option<String>) -> Result<Option<DateTime<Utc>>, AppError> {
    let Some(value) = value.as_deref() else {
        return Ok(None);
    };

    if !value.ends_with('Z') {
        return Err(AppError::bad_request(
            "invalid_utc_time",
            "timestamps must be RFC3339 UTC format ending with Z",
        ));
    }

    let parsed = DateTime::parse_from_rfc3339(value).map_err(|_| {
        AppError::bad_request(
            "invalid_utc_time",
            "timestamps must be RFC3339 UTC format ending with Z",
        )
    })?;

    if parsed.offset().local_minus_utc() != 0 {
        return Err(AppError::bad_request(
            "invalid_utc_time",
            "timestamps must use UTC offset",
        ));
    }

    Ok(Some(parsed.with_timezone(&Utc)))
}

fn normalize_priority(priority: Option<String>) -> Result<Option<String>, AppError> {
    let Some(value) = priority else {
        return Ok(None);
    };

    let normalized = value.trim().to_ascii_lowercase();
    if normalized.is_empty() {
        return Err(AppError::bad_request(
            "invalid_priority",
            "priority must be one of 0-7 or: emerg, alert, crit, err, warning, notice, info, debug",
        ));
    }

    let mapped = match normalized.as_str() {
        "0" | "emerg" | "panic" => "0",
        "1" | "alert" => "1",
        "2" | "crit" | "critical" => "2",
        "3" | "err" | "error" => "3",
        "4" | "warning" | "warn" => "4",
        "5" | "notice" => "5",
        "6" | "info" | "informational" => "6",
        "7" | "debug" => "7",
        _ => return Err(AppError::bad_request(
            "invalid_priority",
            "priority must be one of 0-7 or: emerg, alert, crit, err, warning, notice, info, debug",
        )),
    };

    Ok(Some(mapped.to_string()))
}

fn normalize_unit(unit: Option<String>) -> Result<Option<String>, AppError> {
    let Some(value) = unit else {
        return Ok(None);
    };

    let normalized = value.trim();
    if normalized.is_empty() {
        return Err(AppError::bad_request(
            "invalid_unit",
            "unit must contain only alphanumeric characters, dashes, underscores, dots, @, and :",
        ));
    }

    if !normalized.chars().all(|character| {
        character.is_ascii_alphanumeric()
            || character == '-'
            || character == '_'
            || character == '@'
            || character == ':'
            || character == '.'
    }) {
        return Err(AppError::bad_request(
            "invalid_unit",
            "unit must contain only alphanumeric characters, dashes, underscores, dots, @, and :",
        ));
    }

    Ok(Some(normalized.to_string()))
}

fn normalize_service_state(state: Option<String>) -> Result<Option<String>, AppError> {
    let Some(value) = state else {
        return Ok(None);
    };

    let normalized = value.trim().to_ascii_lowercase();
    if normalized.is_empty() {
        return Err(AppError::bad_request(
            "invalid_state",
            "state must be one of: active, inactive, failed, activating, deactivating, reloading",
        ));
    }

    if !VALID_SERVICE_STATES.contains(&normalized.as_str()) {
        return Err(AppError::bad_request(
            "invalid_state",
            "state must be one of: active, inactive, failed, activating, deactivating, reloading",
        ));
    }

    Ok(Some(normalized))
}

fn filter_services_by_state(services: Vec<UnitStatus>, state: Option<&str>) -> Vec<UnitStatus> {
    let Some(state) = state else {
        return services;
    };

    services
        .into_iter()
        .filter(|service| service.state.eq_ignore_ascii_case(state))
        .collect()
}

fn json_rpc_error(id: Option<Value>, code: i32, message: &str) -> Value {
    json_rpc_error_with_data(id, code, message, None)
}

fn json_rpc_error_with_data(
    id: Option<Value>,
    code: i32,
    message: &str,
    data: Option<Value>,
) -> Value {
    let response = JsonrpcErrorResponse::new(
        RpcError {
            code: i64::from(code),
            data,
            message: message.to_string(),
        },
        id.as_ref().and_then(value_to_request_id),
    );
    serde_json::to_value(response).expect("jsonrpc error response serialization")
}

fn json_rpc_result(id: Option<Value>, result: Value) -> Value {
    if let Some(request_id) = id.as_ref().and_then(value_to_request_id) {
        let extra = result.as_object().cloned();
        let response = JsonrpcResultResponse::new(request_id, McpResult { meta: None, extra });
        return serde_json::to_value(response).expect("jsonrpc result response serialization");
    }

    json!({
        "jsonrpc": "2.0",
        "id": id,
        "result": result
    })
}

fn value_to_request_id(value: &Value) -> Option<RequestId> {
    if let Some(string_id) = value.as_str() {
        return Some(RequestId::String(string_id.to_string()));
    }

    value.as_i64().map(RequestId::Integer)
}

fn request_id_to_value(id: RequestId) -> Value {
    match id {
        RequestId::String(value) => Value::String(value),
        RequestId::Integer(value) => Value::Number(value.into()),
    }
}

#[cfg(test)]
mod tests {
    use crate::systemd_client::UnitStatus;
    use serde_json::json;

    use super::{
        build_log_query, filter_services_by_state, negotiate_protocol_version,
        normalize_service_state, redact_audit_params, LogsQueryParams, MAX_LOG_LIMIT,
    };

    #[test]
    fn rejects_limit_above_max() {
        let query = build_log_query(LogsQueryParams {
            priority: None,
            unit: None,
            start_utc: None,
            end_utc: None,
            limit: Some(MAX_LOG_LIMIT + 1),
        });

        let error = query.expect_err("expected invalid limit");
        assert!(error.to_string().contains("bad request"));
    }

    #[test]
    fn rejects_non_utc_time() {
        let query = build_log_query(LogsQueryParams {
            priority: None,
            unit: None,
            start_utc: Some("2026-02-27T12:00:00+01:00".to_string()),
            end_utc: Some("2026-02-27T13:00:00Z".to_string()),
            limit: Some(10),
        });

        let error = query.expect_err("expected invalid utc time");
        assert!(error.to_string().contains("bad request"));
    }

    #[test]
    fn normalizes_priority_alias() {
        let query = build_log_query(LogsQueryParams {
            priority: Some("error".to_string()),
            unit: Some("ssh_service-01@host:prod".to_string()),
            start_utc: Some("2026-02-27T00:00:00Z".to_string()),
            end_utc: Some("2026-02-27T01:00:00Z".to_string()),
            limit: Some(10),
        })
        .expect("query should build");

        assert_eq!(query.priority.as_deref(), Some("3"));
        assert_eq!(query.unit.as_deref(), Some("ssh_service-01@host:prod"));
    }

    #[test]
    fn rejects_unit_with_disallowed_characters() {
        let query = build_log_query(LogsQueryParams {
            priority: None,
            unit: Some("sshd/service".to_string()),
            start_utc: Some("2026-02-27T00:00:00Z".to_string()),
            end_utc: Some("2026-02-27T01:00:00Z".to_string()),
            limit: Some(10),
        });

        let error = query.expect_err("expected invalid unit");
        assert!(error.to_string().contains("bad request"));
    }

    #[test]
    fn rejects_missing_time_range() {
        let query = build_log_query(LogsQueryParams {
            priority: None,
            unit: None,
            start_utc: None,
            end_utc: None,
            limit: Some(10),
        });

        let error = query.expect_err("expected missing time range");
        assert!(error.to_string().contains("bad request"));
    }

    #[test]
    fn normalizes_service_state() {
        let state = normalize_service_state(Some(" FaILeD ".to_string())).expect("valid state");
        assert_eq!(state.as_deref(), Some("failed"));
    }

    #[test]
    fn rejects_invalid_service_state() {
        let state = normalize_service_state(Some("running".to_string()));
        let error = state.expect_err("expected invalid state");
        assert!(error.to_string().contains("bad request"));
    }

    #[test]
    fn filters_services_by_state_case_insensitive() {
        let services = vec![
            UnitStatus {
                name: "a.service".to_string(),
                state: "active".to_string(),
                description: None,
            },
            UnitStatus {
                name: "b.service".to_string(),
                state: "failed".to_string(),
                description: None,
            },
        ];

        let filtered = filter_services_by_state(services, Some("FaIlEd"));
        assert_eq!(filtered.len(), 1);
        assert_eq!(filtered[0].name, "b.service");
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
            "protocolVersion": "2024-11-05"
        });

        let version = negotiate_protocol_version(Some(&params)).expect("supported version");
        assert_eq!(version, super::ProtocolVersion::V2024_11_05);
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
