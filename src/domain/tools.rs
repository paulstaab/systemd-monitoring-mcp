//! Interactive tools exposed via Model Context Protocol
//!
//! Provides MCP tool catalog and dispatch for service, timer, and log monitoring.

mod logs;
mod services;
mod timers;

use rust_mcp_sdk::{
    macros,
    schema::{CallToolRequestParams, Tool},
};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::mcp::rpc::{json_rpc_invalid_params, json_rpc_method_not_found_with_data};
use crate::AppState;

pub use logs::build_log_query;
pub use timers::{parse_timers_query_params, sort_timer_items, TimerItem};

#[derive(Debug, Deserialize)]
pub struct ServicesQueryParams {
    pub state: Option<String>,
    pub name_contains: Option<String>,
    pub limit: Option<u32>,
    pub summary: Option<bool>,
}

#[derive(Debug, Deserialize)]
pub struct LogsQueryParams {
    pub priority: Option<String>,
    pub unit: Option<String>,
    pub start_utc: Option<String>,
    pub end_utc: Option<String>,
    pub grep: Option<String>,
    pub exclude_units: Option<Vec<String>>,
    pub order: Option<String>,
    pub allow_large_window: Option<bool>,
    pub limit: Option<u32>,
    pub summary: Option<bool>,
}

#[macros::mcp_tool(
    name = "list_services",
    description = "List systemd service units and current state"
)]
#[derive(Debug, Deserialize, Serialize, macros::JsonSchema)]
pub struct ListServicesTool {
    pub state: Option<String>,
    pub name_contains: Option<String>,
    pub limit: Option<u32>,
    pub summary: Option<bool>,
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
    pub grep: Option<String>,
    pub exclude_units: Option<Vec<String>>,
    pub order: Option<String>,
    pub allow_large_window: Option<bool>,
    pub limit: Option<u32>,
    pub summary: Option<bool>,
}

#[macros::mcp_tool(
    name = "list_timers",
    description = "List systemd timer units and scheduling/trigger state"
)]
#[derive(Debug, Deserialize, Serialize, macros::JsonSchema)]
pub struct ListTimersTool {
    pub limit: Option<u32>,
    pub name_contains: Option<String>,
    pub state: Option<String>,
    pub summary: Option<bool>,
    pub include_persistent: Option<bool>,
    pub overdue_only: Option<bool>,
    pub sort: Option<String>,
    pub order: Option<String>,
}

/// Builds the advertised MCP tool catalog returned by `tools/list`.
pub fn build_tools_list() -> Vec<Tool> {
    vec![
        ListServicesTool::tool(),
        ListTimersTool::tool(),
        ListLogsTool::tool(),
    ]
}

/// Handles MCP `tools/call` requests and dispatches to supported tool handlers.
///
/// Returns JSON-RPC `-32602` for malformed params and `-32601` with structured
/// tool details for unknown tool names.
pub async fn handle_tools_call(
    state: &AppState,
    id: Option<Value>,
    params: Option<Value>,
) -> Value {
    let Some(raw_params) = params else {
        return json_rpc_invalid_params(id);
    };

    let tool_call: CallToolRequestParams = match serde_json::from_value(raw_params) {
        Ok(value) => value,
        Err(_) => return json_rpc_invalid_params(id),
    };

    match tool_call.name.as_str() {
        "list_services" => services::handle_list_services(state, id, tool_call.arguments).await,
        "list_timers" => timers::handle_list_timers(state, id, tool_call.arguments).await,
        "list_logs" => logs::handle_list_logs(state, id, tool_call.arguments).await,
        _ => json_rpc_method_not_found_with_data(
            id,
            json!({
                "code": "tool_not_found",
                "message": "unknown tool name",
                "details": {
                    "name": tool_call.name,
                },
            }),
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::{
        build_log_query, parse_timers_query_params, sort_timer_items, LogsQueryParams, TimerItem,
    };
    use crate::domain::utils::MAX_LOG_LIMIT;
    use serde_json::json;

    #[test]
    fn rejects_limit_above_max() {
        let query = build_log_query(LogsQueryParams {
            priority: None,
            unit: None,
            start_utc: None,
            end_utc: None,
            grep: None,
            exclude_units: None,
            order: None,
            allow_large_window: None,
            limit: Some((MAX_LOG_LIMIT + 1) as u32),
            summary: None,
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
            grep: None,
            exclude_units: None,
            order: None,
            allow_large_window: None,
            limit: Some(10),
            summary: None,
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
            grep: None,
            exclude_units: None,
            order: None,
            allow_large_window: None,
            limit: Some(10),
            summary: None,
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
            grep: None,
            exclude_units: None,
            order: None,
            allow_large_window: None,
            limit: Some(10),
            summary: None,
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
            grep: None,
            exclude_units: None,
            order: None,
            allow_large_window: None,
            limit: Some(10),
            summary: None,
        });

        let error = query.expect_err("expected missing time range");
        assert!(error.to_string().contains("bad request"));
    }

    #[test]
    fn rejects_too_large_time_range_without_override() {
        let query = build_log_query(LogsQueryParams {
            priority: None,
            unit: None,
            start_utc: Some("2026-02-01T00:00:00Z".to_string()),
            end_utc: Some("2026-02-10T00:00:00Z".to_string()),
            grep: None,
            exclude_units: None,
            order: None,
            allow_large_window: None,
            limit: Some(10),
            summary: None,
        });

        let error = query.expect_err("expected too large range");
        assert!(error.to_string().contains("bad request"));
    }

    #[test]
    fn rejects_invalid_list_timers_bool_type() {
        let parsed = parse_timers_query_params(
            json!({
                "summary": "yes"
            })
            .as_object()
            .cloned(),
        );

        let error = parsed.expect_err("expected invalid params type");
        assert!(error.to_string().contains("bad request"));
    }

    #[test]
    fn sorts_next_none_last_for_asc() {
        let mut timers = vec![
            TimerItem {
                unit: "unknown.timer".to_string(),
                active_state: "active".to_string(),
                sub_state: "waiting".to_string(),
                next_run_utc: None,
                last_run_utc: None,
                time_until_next_sec: None,
                time_since_last_sec: None,
                trigger_unit: None,
                persistent: None,
                result: None,
                load_state: Some("loaded".to_string()),
                unit_file_state: Some("enabled".to_string()),
                overdue: false,
                overdue_reason: Some("no_next_run_known".to_string()),
            },
            TimerItem {
                unit: "soon.timer".to_string(),
                active_state: "active".to_string(),
                sub_state: "waiting".to_string(),
                next_run_utc: Some("2099-01-01T00:00:00.000Z".to_string()),
                last_run_utc: None,
                time_until_next_sec: Some(10),
                time_since_last_sec: None,
                trigger_unit: None,
                persistent: None,
                result: None,
                load_state: Some("loaded".to_string()),
                unit_file_state: Some("enabled".to_string()),
                overdue: false,
                overdue_reason: None,
            },
        ];

        sort_timer_items(&mut timers, "next", "asc");

        assert_eq!(timers[0].unit, "soon.timer");
        assert_eq!(timers[1].unit, "unknown.timer");
    }

    #[test]
    fn sorts_last_none_last_for_desc() {
        let mut timers = vec![
            TimerItem {
                unit: "unknown.timer".to_string(),
                active_state: "active".to_string(),
                sub_state: "waiting".to_string(),
                next_run_utc: None,
                last_run_utc: None,
                time_until_next_sec: None,
                time_since_last_sec: None,
                trigger_unit: None,
                persistent: None,
                result: None,
                load_state: Some("loaded".to_string()),
                unit_file_state: Some("enabled".to_string()),
                overdue: false,
                overdue_reason: None,
            },
            TimerItem {
                unit: "recent.timer".to_string(),
                active_state: "active".to_string(),
                sub_state: "waiting".to_string(),
                next_run_utc: None,
                last_run_utc: Some("2026-01-01T00:00:00.000Z".to_string()),
                time_until_next_sec: None,
                time_since_last_sec: Some(100),
                trigger_unit: None,
                persistent: None,
                result: None,
                load_state: Some("loaded".to_string()),
                unit_file_state: Some("enabled".to_string()),
                overdue: false,
                overdue_reason: None,
            },
            TimerItem {
                unit: "older.timer".to_string(),
                active_state: "active".to_string(),
                sub_state: "waiting".to_string(),
                next_run_utc: None,
                last_run_utc: Some("2026-01-01T00:00:00.000Z".to_string()),
                time_until_next_sec: None,
                time_since_last_sec: Some(200),
                trigger_unit: None,
                persistent: None,
                result: None,
                load_state: Some("loaded".to_string()),
                unit_file_state: Some("enabled".to_string()),
                overdue: false,
                overdue_reason: None,
            },
        ];

        sort_timer_items(&mut timers, "last", "desc");

        assert_eq!(timers[0].unit, "older.timer");
        assert_eq!(timers[1].unit, "recent.timer");
        assert_eq!(timers[2].unit, "unknown.timer");
    }
}
