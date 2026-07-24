use chrono::Duration;
use serde::Serialize;
use serde_json::{json, Value};
use std::collections::{BTreeMap, HashMap};

use crate::domain::responses::{generated_at_utc_string, tool_success_response};
use crate::domain::utils::{
    normalize_priority, normalize_scope, normalize_unit, parse_utc, DEFAULT_LOG_LIMIT,
    MAX_LOG_LIMIT,
};
use crate::mcp::rpc::{app_error_to_json_rpc, json_rpc_invalid_params};
use crate::{
    errors::AppError,
    systemd_client::{LogOrder, LogQuery, UnitScope},
    AppState,
};

use super::LogsQueryParams;

#[derive(Debug)]
struct NormalizedLogsQuery {
    query: LogQuery,
    summary_enabled: bool,
    fields: Vec<String>,
    group_by_message: bool,
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

/// Validates the optional log field projection and rejects duplicates.
fn normalize_fields(fields: Option<Vec<String>>) -> Result<Vec<String>, AppError> {
    const ALL: [&str; 7] = [
        "timestamp_utc",
        "unit",
        "priority",
        "hostname",
        "pid",
        "message",
        "cursor",
    ];
    let fields = fields.unwrap_or_else(|| ALL.iter().map(|value| (*value).to_string()).collect());
    if fields.is_empty() {
        return Err(AppError::bad_request(
            "invalid_fields",
            "fields must not be empty",
        ));
    }
    let mut seen = std::collections::HashSet::new();
    for field in &fields {
        if !ALL.contains(&field.as_str()) || !seen.insert(field.clone()) {
            return Err(AppError::bad_request(
                "invalid_fields",
                "fields must be unique supported log fields",
            ));
        }
    }
    Ok(fields)
}

/// Validates page-local grouping selection.
fn normalize_group_by(group_by: Option<String>) -> Result<bool, AppError> {
    match group_by.as_deref() {
        None => Ok(false),
        Some("message") => Ok(true),
        _ => Err(AppError::bad_request(
            "invalid_group_by",
            "group_by must be message",
        )),
    }
}

/// Projects one typed journal row into requested public fields.
fn project_entry(
    entry: &crate::systemd_client::JournalLogEntry,
    fields: &[String],
) -> serde_json::Map<String, Value> {
    let full = serde_json::to_value(entry).expect("journal entry serialization");
    fields
        .iter()
        .filter_map(|field| full.get(field).cloned().map(|value| (field.clone(), value)))
        .collect()
}

/// Groups a raw page by `(unit, priority, message)` while preserving first-seen order.
fn group_entries(
    entries: &[crate::systemd_client::JournalLogEntry],
    fields: &[String],
) -> Vec<Value> {
    type Group = (
        Option<String>,
        Option<String>,
        Option<String>,
        usize,
        String,
        String,
        serde_json::Map<String, Value>,
    );
    let mut groups: Vec<Group> = Vec::new();
    for entry in entries {
        if let Some(group) = groups.iter_mut().find(|group| {
            group.0 == entry.unit && group.1 == entry.priority && group.2 == entry.message
        }) {
            group.3 += 1;
            if entry.timestamp_utc < group.4 {
                group.4 = entry.timestamp_utc.clone();
            }
            if entry.timestamp_utc > group.5 {
                group.5 = entry.timestamp_utc.clone();
            }
        } else {
            groups.push((
                entry.unit.clone(),
                entry.priority.clone(),
                entry.message.clone(),
                1,
                entry.timestamp_utc.clone(),
                entry.timestamp_utc.clone(),
                project_entry(entry, fields),
            ));
        }
    }
    groups
        .into_iter()
        .map(|(_, _, _, count, first, last, mut row)| {
            row.insert("count".to_string(), json!(count));
            row.insert("first_timestamp_utc".to_string(), json!(first));
            row.insert("last_timestamp_utc".to_string(), json!(last));
            Value::Object(row)
        })
        .collect()
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
            return Err(AppError::bad_request_with_details(
                "time_range_too_large",
                "time window must not exceed 7 days unless allow_large_window is true",
                json!({"maximum_start_utc": (*end - seven_days).to_rfc3339_opts(chrono::SecondsFormat::Millis, true)}),
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
        scope: normalize_scope(params.scope)?,
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
        cursor: params.cursor.filter(|value| !value.trim().is_empty()),
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
    let fields =
        normalize_fields(query_params.fields.clone()).map_err(NormalizeLogsError::Domain)?;
    let group_by_message =
        normalize_group_by(query_params.group_by.clone()).map_err(NormalizeLogsError::Domain)?;
    let query = build_log_query(query_params).map_err(NormalizeLogsError::Domain)?;

    Ok(NormalizedLogsQuery {
        query,
        summary_enabled,
        fields,
        group_by_message,
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
    let mut arguments = arguments.unwrap_or_default();
    if arguments.get("since_last_start").and_then(Value::as_bool) == Some(true) {
        if arguments.contains_key("start_utc") {
            return app_error_to_json_rpc(
                id,
                AppError::bad_request(
                    "invalid_time_range",
                    "start_utc must be omitted with since_last_start",
                ),
            );
        }
        let unit = match normalize_unit(
            arguments
                .get("unit")
                .and_then(Value::as_str)
                .map(str::to_string),
        ) {
            Ok(Some(unit)) => unit,
            _ => {
                return app_error_to_json_rpc(
                    id,
                    AppError::bad_request(
                        "invalid_unit",
                        "since_last_start requires exactly one unit",
                    ),
                )
            }
        };
        let scope = match normalize_scope(
            arguments
                .get("scope")
                .and_then(Value::as_str)
                .map(str::to_string),
        ) {
            Ok(UnitScope::System) => UnitScope::System,
            Ok(UnitScope::User) => UnitScope::User,
            _ => {
                return app_error_to_json_rpc(
                    id,
                    AppError::bad_request(
                        "invalid_scope",
                        "since_last_start requires one unit and system or user scope",
                    ),
                )
            }
        };
        match state.unit_provider.unit_main_start(&unit, scope).await {
            Ok(Some(start)) => {
                arguments.insert(
                    "start_utc".to_string(),
                    json!(start.to_rfc3339_opts(chrono::SecondsFormat::Millis, true)),
                );
            }
            Ok(None) => {
                return app_error_to_json_rpc(
                    id,
                    AppError::bad_request(
                        "unit_start_unavailable",
                        "unit main-process start is unavailable",
                    ),
                )
            }
            Err(err) => return app_error_to_json_rpc(id, err),
        }
    }
    let normalized = match normalize_logs_query(Some(arguments)) {
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
            let truncated = log_result.has_more;
            let next_cursor = if truncated {
                log_result
                    .entries
                    .last()
                    .and_then(|entry| entry.cursor.clone())
            } else {
                None
            };
            let detailed_rows = if normalized.group_by_message {
                group_entries(&log_result.entries, &normalized.fields)
            } else {
                log_result
                    .entries
                    .iter()
                    .map(|entry| Value::Object(project_entry(entry, &normalized.fields)))
                    .collect::<Vec<_>>()
            };
            let returned = if normalized.group_by_message {
                detailed_rows.len()
            } else {
                log_result.entries.len()
            };
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
("returned".to_string(), json!(log_result.entries.len())),
                        ("truncated".to_string(), json!(truncated)),
                        ("next_cursor".to_string(), json!(next_cursor)),
                        ("generated_at_utc".to_string(), json!(generated_at_utc)),
                        ("window".to_string(), Value::Object(window)),
                    ]),
                );
            }

            tool_success_response(
                id,
                format!("Returned {returned} log entries"),
                serde_json::Map::from_iter([
                    (
                        (if normalized.group_by_message {
                            "groups"
                        } else {
                            "logs"
                        })
                        .to_string(),
                        json!(detailed_rows),
                    ),
                    ("total_scanned".to_string(), json!(log_result.total_scanned)),
                    ("returned".to_string(), json!(returned)),
                    ("truncated".to_string(), json!(truncated)),
                    ("next_cursor".to_string(), json!(next_cursor)),
                    ("generated_at_utc".to_string(), json!(generated_at_utc)),
                    ("window".to_string(), Value::Object(window)),
                ]),
            )
        }
        Err(err) => app_error_to_json_rpc(id, err),
    }
}
