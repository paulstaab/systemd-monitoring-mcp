use chrono::Duration;
use serde::Serialize;
use serde_json::{json, Value};
use std::collections::{BTreeMap, HashMap};

use crate::domain::responses::{generated_at_utc_string, tool_success_response};
use crate::domain::utils::{
    normalize_priority, normalize_unit, parse_utc, DEFAULT_LOG_LIMIT, MAX_LOG_LIMIT,
};
use crate::mcp::rpc::{app_error_to_json_rpc, json_rpc_invalid_params};
use crate::{
    errors::AppError,
    systemd_client::{LogOrder, LogQuery},
    AppState,
};

use super::LogsQueryParams;

#[derive(Debug)]
struct NormalizedLogsQuery {
    query: LogQuery,
    summary_enabled: bool,
}

enum NormalizeLogsError {
    InvalidParams,
    Domain(AppError),
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

/// Builds `list_logs` summary payload for triage mode.
///
/// Produces top-unit and priority counts, frequent messages, and error hotspots.
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

/// Validates and normalizes `list_logs` query parameters into an execution query.
///
/// Enforces required time range, UTC semantics, time-window bounds, limit caps,
/// unit validation, and supported sort order.
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
        let seven_days = Duration::days(7);
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

/// Parses and normalizes `list_logs` arguments into a typed execution query.
///
/// This consolidates JSON shape validation and domain normalization before
/// provider execution to keep handler control flow minimal and consistent.
fn normalize_logs_query(
    arguments: Option<serde_json::Map<String, Value>>,
) -> Result<NormalizedLogsQuery, NormalizeLogsError> {
    let query_params: LogsQueryParams =
        serde_json::from_value(json!(arguments.unwrap_or_default()))
            .map_err(|_| NormalizeLogsError::InvalidParams)?;
    let summary_enabled = query_params.summary.unwrap_or(false);
    let query = build_log_query(query_params).map_err(NormalizeLogsError::Domain)?;

    Ok(NormalizedLogsQuery {
        query,
        summary_enabled,
    })
}

/// Handles `list_logs` tool execution.
///
/// Parses and validates tool arguments, executes journald query via the provider,
/// and returns either detailed entries or summary triage output.
pub async fn handle_list_logs(
    state: &AppState,
    id: Option<Value>,
    arguments: Option<serde_json::Map<String, Value>>,
) -> Value {
    let normalized = match normalize_logs_query(arguments) {
        Ok(value) => value,
        Err(NormalizeLogsError::InvalidParams) => return json_rpc_invalid_params(id),
        Err(NormalizeLogsError::Domain(err)) => return app_error_to_json_rpc(id, err),
    };

    match state
        .unit_provider
        .list_journal_logs(&normalized.query)
        .await
    {
        Ok(log_result) => {
            let returned = log_result.entries.len();
            let truncated = returned >= normalized.query.limit;
            let generated_at_utc = generated_at_utc_string();
            let window = serde_json::Map::from_iter([
                (
                    "start_utc".to_string(),
                    json!(normalized
                        .query
                        .start_utc
                        .expect("validated start_utc")
                        .to_rfc3339_opts(chrono::SecondsFormat::Millis, true)),
                ),
                (
                    "end_utc".to_string(),
                    json!(normalized
                        .query
                        .end_utc
                        .expect("validated end_utc")
                        .to_rfc3339_opts(chrono::SecondsFormat::Millis, true)),
                ),
            ]);

            if normalized.summary_enabled {
                let summary = build_log_summary(&log_result.entries);
                return tool_success_response(
                    id,
                    "Returned logs triage summary".to_string(),
                    serde_json::Map::from_iter([
                        ("summary".to_string(), json!(summary)),
                        ("total_scanned".to_string(), json!(log_result.total_scanned)),
                        ("returned".to_string(), json!(returned)),
                        ("truncated".to_string(), json!(truncated)),
                        ("generated_at_utc".to_string(), json!(generated_at_utc)),
                        ("window".to_string(), Value::Object(window)),
                    ]),
                );
            }

            tool_success_response(
                id,
                format!("Returned {returned} log entries"),
                serde_json::Map::from_iter([
                    ("logs".to_string(), json!(log_result.entries)),
                    ("total_scanned".to_string(), json!(log_result.total_scanned)),
                    ("returned".to_string(), json!(returned)),
                    ("truncated".to_string(), json!(truncated)),
                    ("generated_at_utc".to_string(), json!(generated_at_utc)),
                    ("window".to_string(), Value::Object(window)),
                ]),
            )
        }
        Err(err) => app_error_to_json_rpc(id, err),
    }
}
