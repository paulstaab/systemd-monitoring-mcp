//! Interactive tools exposed via Model Context Protocol
//!
//! Provides `list_services` and `list_logs` implementations by delegating to
//! the `UnitProvider` systemd implementation dynamically.

use rust_mcp_sdk::{
    macros,
    schema::{CallToolRequestParams, CallToolResult, ContentBlock, TextContent, Tool},
};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::domain::utils::{
    filter_services_by_state, normalize_priority, normalize_service_state, normalize_unit,
    parse_utc, DEFAULT_LOG_LIMIT, MAX_LOG_LIMIT,
};
use crate::mcp::rpc::{
    app_error_to_json_rpc, json_rpc_error, json_rpc_error_with_data, json_rpc_result,
};
use crate::{errors::AppError, systemd_client::LogQuery, AppState};

#[derive(Debug, Deserialize)]
pub struct ServicesQueryParams {
    pub state: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct LogsQueryParams {
    pub priority: Option<String>,
    pub unit: Option<String>,
    pub start_utc: Option<String>,
    pub end_utc: Option<String>,
    pub limit: Option<usize>,
}

#[macros::mcp_tool(
    name = "list_services",
    description = "List systemd service units and current state"
)]
#[derive(Debug, Deserialize, Serialize, macros::JsonSchema)]
pub struct ListServicesTool {
    pub state: Option<String>,
}

#[macros::mcp_tool(
    name = "list_logs",
    description = "List journald logs with filters and bounds"
)]
#[derive(Debug, Deserialize, Serialize, macros::JsonSchema)]
pub struct ListLogsTool {
    pub priority: Option<String>,
    pub unit: Option<String>,
    pub start_utc: String,
    pub end_utc: String,
    pub limit: Option<u32>,
}

pub fn build_tools_list() -> Vec<Tool> {
    vec![ListServicesTool::tool(), ListLogsTool::tool()]
}

pub fn build_log_query(params: LogsQueryParams) -> Result<LogQuery, AppError> {
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

pub async fn handle_tools_call(
    state: &AppState,
    id: Option<Value>,
    params: Option<Value>,
) -> Value {
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

#[cfg(test)]
mod tests {
    use super::{build_log_query, LogsQueryParams};
    use crate::domain::utils::MAX_LOG_LIMIT;

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
}
