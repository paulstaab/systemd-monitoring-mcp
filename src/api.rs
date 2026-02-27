use axum::{body::Bytes, extract::Query, extract::State, Json};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::{
    errors::AppError,
    systemd_client::{JournalLogEntry, LogQuery, LogSortOrder, UnitStatus},
    AppState,
};

const MAX_LOG_LIMIT: usize = 1_000;
const DEFAULT_LOG_LIMIT: usize = 100;

#[derive(Debug, Deserialize)]
pub struct LogsQueryParams {
    priority: Option<String>,
    unit: Option<String>,
    start_utc: Option<String>,
    end_utc: Option<String>,
    limit: Option<usize>,
    order: Option<String>,
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
    pub services_endpoint: &'static str,
    pub logs_endpoint: &'static str,
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
        logs_endpoint: "/logs",
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
                        "services": "/services",
                        "logs": "/logs"
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

pub async fn list_logs(
    State(state): State<AppState>,
    Query(params): Query<LogsQueryParams>,
) -> Result<Json<Vec<JournalLogEntry>>, AppError> {
    let query = build_log_query(params)?;
    let logs = state.unit_provider.list_journal_logs(&query).await?;
    Ok(Json(logs))
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

    let order_value = params.order.map(|order| order.to_ascii_lowercase());
    let order = match order_value.as_deref() {
        None | Some("asc") => LogSortOrder::Asc,
        Some("desc") => LogSortOrder::Desc,
        Some(_) => {
            return Err(AppError::bad_request(
                "invalid_order",
                "order must be one of: asc, desc",
            ))
        }
    };

    Ok(LogQuery {
        priority: normalize_priority(params.priority)?,
        unit: normalize_unit(params.unit)?,
        start_utc,
        end_utc,
        limit,
        order,
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

#[cfg(test)]
mod tests {
    use super::{build_log_query, LogSortOrder, LogsQueryParams, MAX_LOG_LIMIT};

    #[test]
    fn rejects_limit_above_max() {
        let query = build_log_query(LogsQueryParams {
            priority: None,
            unit: None,
            start_utc: None,
            end_utc: None,
            limit: Some(MAX_LOG_LIMIT + 1),
            order: None,
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
            order: None,
        });

        let error = query.expect_err("expected invalid utc time");
        assert!(error.to_string().contains("bad request"));
    }

    #[test]
    fn normalizes_priority_alias_and_desc_order() {
        let query = build_log_query(LogsQueryParams {
            priority: Some("error".to_string()),
            unit: Some("ssh_service-01@host:prod".to_string()),
            start_utc: Some("2026-02-27T00:00:00Z".to_string()),
            end_utc: Some("2026-02-27T01:00:00Z".to_string()),
            limit: Some(10),
            order: Some("DESC".to_string()),
        })
        .expect("query should build");

        assert_eq!(query.priority.as_deref(), Some("3"));
        assert_eq!(query.unit.as_deref(), Some("ssh_service-01@host:prod"));
        assert!(matches!(query.order, LogSortOrder::Desc));
    }

    #[test]
    fn rejects_unit_with_disallowed_characters() {
        let query = build_log_query(LogsQueryParams {
            priority: None,
            unit: Some("sshd/service".to_string()),
            start_utc: Some("2026-02-27T00:00:00Z".to_string()),
            end_utc: Some("2026-02-27T01:00:00Z".to_string()),
            limit: Some(10),
            order: None,
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
            order: None,
        });

        let error = query.expect_err("expected missing time range");
        assert!(error.to_string().contains("bad request"));
    }
}
