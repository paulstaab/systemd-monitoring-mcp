//! Interactive tools exposed via Model Context Protocol
//!
//! Provides `list_services` and `list_logs` implementations by delegating to
//! the `UnitProvider` systemd implementation dynamically.

use chrono::{SecondsFormat, Utc};
use rust_mcp_sdk::{
    macros,
    schema::{CallToolRequestParams, CallToolResult, ContentBlock, TextContent, Tool},
};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::{BTreeMap, HashMap};

use crate::domain::utils::{
    filter_services_by_name_contains, filter_services_by_state, normalize_name_contains,
    normalize_priority, normalize_service_state, normalize_services_limit, normalize_unit,
    parse_utc, sort_services, DEFAULT_LOG_LIMIT, MAX_LOG_LIMIT,
};
use crate::mcp::rpc::{
    app_error_to_json_rpc, json_rpc_error, json_rpc_error_with_data, json_rpc_result,
};
use crate::{
    errors::AppError,
    systemd_client::{LogOrder, LogQuery},
    AppState,
};

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

#[derive(Debug, Serialize)]
struct FailedUnitSummary {
    unit: String,
    sub_state: String,
    result: Option<String>,
    since_utc: Option<String>,
}

#[derive(Debug, Serialize)]
struct ServiceSummary {
    counts_by_active_state: BTreeMap<String, usize>,
    failed_units: Vec<FailedUnitSummary>,
    degraded_hint: Option<String>,
}

#[derive(Debug, Serialize)]
struct MessageSummary {
    message: String,
    count: usize,
}

#[derive(Debug, Serialize)]
struct ErrorHotspotSummary {
    unit: String,
    error_count: usize,
}

#[derive(Debug, Serialize)]
struct LogSummary {
    counts_by_unit: BTreeMap<String, usize>,
    counts_by_priority: BTreeMap<String, usize>,
    top_messages: Vec<MessageSummary>,
    error_hotspots: Vec<ErrorHotspotSummary>,
}

fn build_service_summary(services: &[crate::systemd_client::UnitStatus]) -> ServiceSummary {
    let mut counts_by_active_state = BTreeMap::new();
    for service in services {
        *counts_by_active_state
            .entry(service.active_state.clone())
            .or_insert(0) += 1;
    }

    let mut failed_units = services
        .iter()
        .filter(|service| service.active_state.eq_ignore_ascii_case("failed"))
        .map(|service| FailedUnitSummary {
            unit: service.unit.clone(),
            sub_state: service.sub_state.clone(),
            result: service.result.clone(),
            since_utc: service.since_utc.clone(),
        })
        .collect::<Vec<_>>();

    failed_units.sort_by(|left, right| left.unit.cmp(&right.unit));
    failed_units.truncate(10);

    let degraded_hint = if failed_units.is_empty() {
        None
    } else {
        Some(format!(
            "Detected {} failed service(s); review failed_units for triage",
            failed_units.len()
        ))
    };

    ServiceSummary {
        counts_by_active_state,
        failed_units,
        degraded_hint,
    }
}

fn build_log_summary(entries: &[crate::systemd_client::JournalLogEntry]) -> LogSummary {
    let mut counts_by_unit_raw: HashMap<String, usize> = HashMap::new();
    let mut counts_by_priority_raw: HashMap<String, usize> = HashMap::new();
    let mut message_counts: HashMap<String, usize> = HashMap::new();
    let mut error_hotspots_raw: HashMap<String, usize> = HashMap::new();

    for entry in entries {
        if let Some(unit) = &entry.unit {
            *counts_by_unit_raw.entry(unit.clone()).or_insert(0) += 1;
        }

        let priority_key = entry
            .priority
            .clone()
            .unwrap_or_else(|| "unknown".to_string());
        *counts_by_priority_raw.entry(priority_key).or_insert(0) += 1;

        if let Some(message) = &entry.message {
            *message_counts.entry(message.clone()).or_insert(0) += 1;
        }

        let is_error = entry
            .priority
            .as_deref()
            .and_then(|value| value.parse::<u8>().ok())
            .map(|priority| priority <= 3)
            .unwrap_or(false);

        if is_error {
            if let Some(unit) = &entry.unit {
                *error_hotspots_raw.entry(unit.clone()).or_insert(0) += 1;
            }
        }
    }

    let mut counts_by_unit_vec = counts_by_unit_raw.into_iter().collect::<Vec<_>>();
    counts_by_unit_vec
        .sort_by(|left, right| right.1.cmp(&left.1).then_with(|| left.0.cmp(&right.0)));
    counts_by_unit_vec.truncate(10);
    let counts_by_unit = BTreeMap::from_iter(counts_by_unit_vec);

    let mut counts_by_priority_vec = counts_by_priority_raw.into_iter().collect::<Vec<_>>();
    counts_by_priority_vec.sort_by(|left, right| left.0.cmp(&right.0));
    let counts_by_priority = BTreeMap::from_iter(counts_by_priority_vec);

    let mut top_messages = message_counts
        .into_iter()
        .map(|(message, count)| MessageSummary { message, count })
        .collect::<Vec<_>>();
    top_messages.sort_by(|left, right| {
        right
            .count
            .cmp(&left.count)
            .then_with(|| left.message.cmp(&right.message))
    });
    top_messages.truncate(10);

    let mut error_hotspots = error_hotspots_raw
        .into_iter()
        .map(|(unit, error_count)| ErrorHotspotSummary { unit, error_count })
        .collect::<Vec<_>>();
    error_hotspots.sort_by(|left, right| {
        right
            .error_count
            .cmp(&left.error_count)
            .then_with(|| left.unit.cmp(&right.unit))
    });

    LogSummary {
        counts_by_unit,
        counts_by_priority,
        top_messages,
        error_hotspots,
    }
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

    if let (Some(start), Some(end)) = (start_utc.as_ref(), end_utc.as_ref()) {
        if start >= end {
            return Err(AppError::bad_request(
                "invalid_time_range",
                "start_utc must be strictly less than end_utc",
            ));
        }

        let allow_large_window = params.allow_large_window.unwrap_or(false);
        let seven_days = chrono::Duration::days(7);
        if !allow_large_window && (*end - *start) > seven_days {
            return Err(AppError::bad_request(
                "time_range_too_large",
                "time window must not exceed 7 days unless allow_large_window is true",
            ));
        }
    }

    let limit = params.limit.unwrap_or(DEFAULT_LOG_LIMIT as u32);
    if limit == 0 || limit > MAX_LOG_LIMIT as u32 {
        return Err(AppError::bad_request(
            "invalid_limit",
            "limit must be between 1 and 1000",
        ));
    }

    let order = match params
        .order
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_ascii_lowercase)
        .as_deref()
    {
        None | Some("desc") => LogOrder::Desc,
        Some("asc") => LogOrder::Asc,
        _ => {
            return Err(AppError::bad_request(
                "invalid_order",
                "order must be one of: asc, desc",
            ))
        }
    };

    let exclude_units = params
        .exclude_units
        .unwrap_or_default()
        .into_iter()
        .map(|unit| normalize_unit(Some(unit)))
        .collect::<Result<Vec<_>, _>>()?
        .into_iter()
        .flatten()
        .collect::<Vec<_>>();

    Ok(LogQuery {
        priority: normalize_priority(params.priority)?,
        unit: normalize_unit(params.unit)?,
        exclude_units,
        grep: params
            .grep
            .map(|value| value.trim().to_string())
            .filter(|value| !value.is_empty()),
        order,
        start_utc,
        end_utc,
        limit: limit as usize,
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
            let name_contains_filter = normalize_name_contains(query_params.name_contains);
            let limit = match normalize_services_limit(query_params.limit) {
                Ok(value) => value,
                Err(err) => return app_error_to_json_rpc(id, err),
            };
            let summary_enabled = query_params.summary.unwrap_or(false);

            match state.unit_provider.list_service_units().await {
                Ok(mut services) => {
                    services = filter_services_by_state(services, state_filter.as_deref());
                    services =
                        filter_services_by_name_contains(services, name_contains_filter.as_deref());

                    let failed_first = state_filter.as_deref() == Some("failed");
                    sort_services(&mut services, failed_first);

                    if summary_enabled {
                        let summary = build_service_summary(&services);
                        let generated_at_utc =
                            Utc::now().to_rfc3339_opts(SecondsFormat::Millis, true);

                        return json_rpc_result(
                            id,
                            serde_json::to_value(CallToolResult {
                                content: vec![ContentBlock::from(TextContent::new(
                                    "Returned service triage summary".to_string(),
                                    None,
                                    None,
                                ))],
                                is_error: None,
                                meta: None,
                                structured_content: Some(serde_json::Map::from_iter([
                                    ("summary".to_string(), json!(summary)),
                                    ("generated_at_utc".to_string(), json!(generated_at_utc)),
                                ])),
                            })
                            .expect("list_services summary serialization"),
                        );
                    }

                    let total = services.len();
                    let services = services.into_iter().take(limit).collect::<Vec<_>>();
                    let returned = services.len();
                    let truncated = total > returned;
                    let generated_at_utc = Utc::now().to_rfc3339_opts(SecondsFormat::Millis, true);

                    json_rpc_result(
                        id,
                        serde_json::to_value(CallToolResult {
                            content: vec![ContentBlock::from(TextContent::new(
                                format!("Returned {returned} of {total} services"),
                                None,
                                None,
                            ))],
                            is_error: None,
                            meta: None,
                            structured_content: Some(serde_json::Map::from_iter([
                                ("services".to_string(), json!(services)),
                                ("total".to_string(), json!(total)),
                                ("returned".to_string(), json!(returned)),
                                ("truncated".to_string(), json!(truncated)),
                                ("generated_at_utc".to_string(), json!(generated_at_utc)),
                            ])),
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

            let summary_enabled = query_params.summary.unwrap_or(false);

            let query = match build_log_query(query_params) {
                Ok(query) => query,
                Err(err) => return app_error_to_json_rpc(id, err),
            };

            match state.unit_provider.list_journal_logs(&query).await {
                Ok(log_result) => {
                    let returned = log_result.entries.len();
                    let truncated = returned >= query.limit;
                    let generated_at_utc = Utc::now().to_rfc3339_opts(SecondsFormat::Millis, true);
                    let window = serde_json::Map::from_iter([
                        (
                            "start_utc".to_string(),
                            json!(query
                                .start_utc
                                .expect("validated start_utc")
                                .to_rfc3339_opts(SecondsFormat::Millis, true)),
                        ),
                        (
                            "end_utc".to_string(),
                            json!(query
                                .end_utc
                                .expect("validated end_utc")
                                .to_rfc3339_opts(SecondsFormat::Millis, true)),
                        ),
                    ]);

                    if summary_enabled {
                        let summary = build_log_summary(&log_result.entries);
                        return json_rpc_result(
                            id,
                            serde_json::to_value(CallToolResult {
                                content: vec![ContentBlock::from(TextContent::new(
                                    "Returned logs triage summary".to_string(),
                                    None,
                                    None,
                                ))],
                                is_error: None,
                                meta: None,
                                structured_content: Some(serde_json::Map::from_iter([
                                    ("summary".to_string(), json!(summary)),
                                    ("total_scanned".to_string(), json!(log_result.total_scanned)),
                                    ("returned".to_string(), json!(returned)),
                                    ("truncated".to_string(), json!(truncated)),
                                    ("generated_at_utc".to_string(), json!(generated_at_utc)),
                                    ("window".to_string(), Value::Object(window)),
                                ])),
                            })
                            .expect("list_logs summary serialization"),
                        );
                    }

                    json_rpc_result(
                        id,
                        serde_json::to_value(CallToolResult {
                            content: vec![ContentBlock::from(TextContent::new(
                                format!("Returned {returned} log entries"),
                                None,
                                None,
                            ))],
                            is_error: None,
                            meta: None,
                            structured_content: Some(serde_json::Map::from_iter([
                                ("logs".to_string(), json!(log_result.entries)),
                                ("total_scanned".to_string(), json!(log_result.total_scanned)),
                                ("returned".to_string(), json!(returned)),
                                ("truncated".to_string(), json!(truncated)),
                                ("generated_at_utc".to_string(), json!(generated_at_utc)),
                                ("window".to_string(), Value::Object(window)),
                            ])),
                        })
                        .expect("list_logs tool result serialization"),
                    )
                }
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
}
