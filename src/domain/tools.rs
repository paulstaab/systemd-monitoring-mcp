//! Interactive tools exposed via Model Context Protocol
//!
//! Provides `list_services` and `list_logs` implementations by delegating to
//! the `UnitProvider` systemd implementation dynamically.

use chrono::{DateTime, Duration, SecondsFormat, Utc};
use rust_mcp_sdk::{
    macros,
    schema::{CallToolRequestParams, CallToolResult, ContentBlock, TextContent, Tool},
};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::cmp::Ordering;
use std::collections::{BTreeMap, HashMap};

use crate::domain::utils::{
    filter_services_by_name_contains, filter_services_by_state, normalize_name_contains,
    normalize_priority, normalize_service_state, normalize_services_limit, normalize_timer_state,
    normalize_timers_limit, normalize_timers_order, normalize_timers_sort, normalize_unit,
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

#[derive(Debug)]
pub struct TimersQueryParams {
    pub limit: Option<u32>,
    pub name_contains: Option<String>,
    pub state: Option<String>,
    pub summary: Option<bool>,
    pub include_persistent: Option<bool>,
    pub overdue_only: Option<bool>,
    pub sort: Option<String>,
    pub order: Option<String>,
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

#[derive(Debug, Serialize, Clone)]
struct TimerItem {
    unit: String,
    active_state: String,
    sub_state: String,
    next_run_utc: Option<String>,
    last_run_utc: Option<String>,
    time_until_next_sec: Option<i64>,
    time_since_last_sec: Option<i64>,
    trigger_unit: Option<String>,
    persistent: Option<bool>,
    result: Option<String>,
    load_state: Option<String>,
    unit_file_state: Option<String>,
    overdue: bool,
    overdue_reason: Option<String>,
}

#[derive(Debug, Serialize)]
struct NextDueSoonTimer {
    unit: String,
    next_run_utc: String,
    time_until_next_sec: i64,
    active_state: String,
    trigger_unit: Option<String>,
}

#[derive(Debug, Serialize)]
struct ProblemTimer {
    unit: String,
    active_state: String,
    sub_state: String,
    result: Option<String>,
    overdue: bool,
    overdue_reason: Option<String>,
}

#[derive(Debug, Serialize)]
struct TimerSummary {
    counts_by_active_state: BTreeMap<String, usize>,
    overdue_count: usize,
    next_due_soon: Vec<NextDueSoonTimer>,
    failed_or_problem_timers: Vec<ProblemTimer>,
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
    vec![
        ListServicesTool::tool(),
        ListTimersTool::tool(),
        ListLogsTool::tool(),
    ]
}

/// Extracts an optional boolean argument from a tool-argument map.
///
/// Returns `Ok(None)` when the key is missing.
/// Returns `invalid_params` when the key exists but is not a JSON boolean.
///
/// Future maintainers:
/// - Keep this strict: silent coercion from strings/numbers makes MCP behavior unpredictable.
/// - Keep error code stable (`invalid_params`) because clients may branch on it.
fn parse_optional_bool_argument(
    arguments: &serde_json::Map<String, Value>,
    key: &str,
) -> Result<Option<bool>, AppError> {
    let Some(value) = arguments.get(key) else {
        return Ok(None);
    };

    value
        .as_bool()
        .map(Some)
        .ok_or_else(|| AppError::bad_request("invalid_params", "boolean parameter expected"))
}

/// Extracts an optional string argument from a tool-argument map.
///
/// Returns `Ok(None)` when the key is missing.
/// Returns `invalid_params` when the key exists but is not a JSON string.
///
/// Future maintainers:
/// - Do not trim/lowercase here; normalization belongs in domain-specific normalizers.
/// - Keeping extraction and normalization separate makes validation behavior easier to test.
fn parse_optional_string_argument(
    arguments: &serde_json::Map<String, Value>,
    key: &str,
) -> Result<Option<String>, AppError> {
    let Some(value) = arguments.get(key) else {
        return Ok(None);
    };

    value
        .as_str()
        .map(|v| Some(v.to_string()))
        .ok_or_else(|| AppError::bad_request("invalid_params", "string parameter expected"))
}

/// Extracts an optional unsigned 32-bit integer argument from a tool-argument map.
///
/// Returns `Ok(None)` when the key is missing.
/// Returns `invalid_params` when the value is not an integer or exceeds `u32` range.
///
/// Future maintainers:
/// - Keep this typed as `u32` because higher-level limit normalizers already enforce business caps.
/// - Avoid accepting floats, even whole-valued floats, to preserve strict JSON schema semantics.
fn parse_optional_u32_argument(
    arguments: &serde_json::Map<String, Value>,
    key: &str,
) -> Result<Option<u32>, AppError> {
    let Some(value) = arguments.get(key) else {
        return Ok(None);
    };

    let Some(raw) = value.as_u64() else {
        return Err(AppError::bad_request(
            "invalid_params",
            "integer parameter expected",
        ));
    };

    u32::try_from(raw)
        .map(Some)
        .map_err(|_| AppError::bad_request("invalid_params", "integer parameter out of range"))
}

/// Parses raw MCP arguments into strongly-typed timer query parameters.
///
/// This parser is intentionally strict about JSON value types and delegates
/// business-rule checks (allowed sort keys, limit ranges, etc.) to the
/// dedicated normalizer functions.
///
/// Future maintainers:
/// - Add new timer arguments here first, then wire normalizers and tests.
/// - Keep this function free of side effects so malformed requests fail fast.
fn parse_timers_query_params(
    arguments: Option<serde_json::Map<String, Value>>,
) -> Result<TimersQueryParams, AppError> {
    let arguments = arguments.unwrap_or_default();

    Ok(TimersQueryParams {
        limit: parse_optional_u32_argument(&arguments, "limit")?,
        name_contains: parse_optional_string_argument(&arguments, "name_contains")?,
        state: parse_optional_string_argument(&arguments, "state")?,
        summary: parse_optional_bool_argument(&arguments, "summary")?,
        include_persistent: parse_optional_bool_argument(&arguments, "include_persistent")?,
        overdue_only: parse_optional_bool_argument(&arguments, "overdue_only")?,
        sort: parse_optional_string_argument(&arguments, "sort")?,
        order: parse_optional_string_argument(&arguments, "order")?,
    })
}

/// Parses an optional RFC3339 timestamp string to UTC.
///
/// Returns `None` for absent or unparsable values. This is used for best-effort
/// enrichment on timer metadata where partial data is acceptable.
///
/// Future maintainers:
/// - Do not surface parsing failures from this helper as hard errors for timer listing;
///   requirements mandate partial results with nulls rather than failing the whole response.
fn parse_rfc3339_utc(value: Option<&str>) -> Option<DateTime<Utc>> {
    value
        .and_then(|timestamp| DateTime::parse_from_rfc3339(timestamp).ok())
        .map(|value| value.with_timezone(&Utc))
}

/// Computes overdue classification for a timer using the project rules.
///
/// Rule summary:
/// - Timer must be `active`.
/// - Timer must have a known `next_run_utc`.
/// - `now` must be strictly later than `next_run_utc + 5 minutes`.
///
/// Returns a tuple of `(overdue, overdue_reason)` where reason is populated for
/// either overdue or explicit non-overdue explanations (e.g. `not_active`).
///
/// Future maintainers:
/// - Keep grace period and reasons synchronized with `docs/requirements.md`.
/// - If grace becomes configurable, centralize config lookup and keep this function pure.
fn timer_overdue_status(
    now: DateTime<Utc>,
    timer: &crate::systemd_client::TimerStatus,
) -> (bool, Option<String>) {
    if !timer.active_state.eq_ignore_ascii_case("active") {
        return (false, Some("not_active".to_string()));
    }

    let Some(next_run) = parse_rfc3339_utc(timer.next_run_utc.as_deref()) else {
        return (false, Some("no_next_run_known".to_string()));
    };

    let grace_deadline = next_run + Duration::minutes(5);
    if now > grace_deadline {
        return (true, Some("past_due_beyond_grace".to_string()));
    }

    (false, None)
}

/// Builds the API-facing timer item with derived fields.
///
/// Derivations include:
/// - `time_until_next_sec`
/// - `time_since_last_sec`
/// - `overdue` / `overdue_reason`
///
/// `persistent` is conditionally included based on `include_persistent` so clients
/// can opt into that field explicitly.
///
/// Future maintainers:
/// - Keep this transformation deterministic and side-effect free for testability.
/// - Preserve nullability semantics required by MCP output contracts.
fn build_timer_item(
    timer: crate::systemd_client::TimerStatus,
    now: DateTime<Utc>,
    include_persistent: bool,
) -> TimerItem {
    let next_run = parse_rfc3339_utc(timer.next_run_utc.as_deref());
    let last_run = parse_rfc3339_utc(timer.last_run_utc.as_deref());

    let time_until_next_sec = next_run.map(|next| (next - now).num_seconds());
    let time_since_last_sec = last_run.map(|last| (now - last).num_seconds());
    let (overdue, overdue_reason) = timer_overdue_status(now, &timer);

    TimerItem {
        unit: timer.unit,
        active_state: timer.active_state,
        sub_state: timer.sub_state,
        next_run_utc: timer.next_run_utc,
        last_run_utc: timer.last_run_utc,
        time_until_next_sec,
        time_since_last_sec,
        trigger_unit: timer.trigger_unit,
        persistent: if include_persistent {
            timer.persistent
        } else {
            None
        },
        result: timer.result,
        load_state: Some(timer.load_state),
        unit_file_state: timer.unit_file_state,
        overdue,
        overdue_reason,
    }
}

/// Sorts timer rows according to `sort` and `order` query options.
///
/// Special behavior for `next` and `last`:
/// - `None` values are always placed last (both asc and desc).
/// - Non-`None` values respect the requested order.
/// - Unit name remains a deterministic tie-breaker.
///
/// Future maintainers:
/// - Keep `None`-last behavior stable; clients use this for triage UX.
/// - If adding new sort keys, ensure tie-breakers remain deterministic.
fn sort_timer_items(items: &mut [TimerItem], sort: &str, order: &str) {
    let is_desc = order.eq_ignore_ascii_case("desc");

    items.sort_by(|left, right| {
        let cmp = match sort {
            "next" => compare_optional_i64_none_last(
                left.time_until_next_sec,
                right.time_until_next_sec,
                is_desc,
            )
            .then_with(|| left.unit.cmp(&right.unit)),
            "last" => compare_optional_i64_none_last(
                left.time_since_last_sec,
                right.time_since_last_sec,
                is_desc,
            )
            .then_with(|| left.unit.cmp(&right.unit)),
            "state" => left
                .active_state
                .cmp(&right.active_state)
                .then_with(|| left.unit.cmp(&right.unit)),
            _ => left.unit.cmp(&right.unit),
        };

        if is_desc && sort != "next" && sort != "last" {
            cmp.reverse()
        } else {
            cmp
        }
    });
}

/// Compares optional integer keys with `None` always ordered last.
///
/// This helper avoids Rust's default `Option` ordering (`None < Some`) because
/// timer sorting semantics require unknown timestamps to sink to the bottom.
fn compare_optional_i64_none_last(
    left: Option<i64>,
    right: Option<i64>,
    descending: bool,
) -> Ordering {
    match (left, right) {
        (None, None) => Ordering::Equal,
        (None, Some(_)) => Ordering::Greater,
        (Some(_), None) => Ordering::Less,
        (Some(left), Some(right)) => {
            if descending {
                right.cmp(&left)
            } else {
                left.cmp(&right)
            }
        }
    }
}

/// Builds compact timer triage summary output for `summary=true` responses.
///
/// Includes:
/// - counts by active state
/// - overdue count
/// - top 5 upcoming timers (`next_due_soon`)
/// - failed/problematic timer list
///
/// Future maintainers:
/// - Keep this output schema stable for MCP clients that parse `structuredContent`.
/// - Revisit truncation limits only with corresponding requirements/test updates.
fn build_timer_summary(items: &[TimerItem]) -> TimerSummary {
    let mut counts_by_active_state = BTreeMap::new();
    for timer in items {
        *counts_by_active_state
            .entry(timer.active_state.clone())
            .or_insert(0) += 1;
    }

    let overdue_count = items.iter().filter(|timer| timer.overdue).count();

    let mut next_due_soon = items
        .iter()
        .filter_map(|timer| {
            let next_run_utc = timer.next_run_utc.as_ref()?;
            let time_until_next_sec = timer.time_until_next_sec?;
            if time_until_next_sec < 0 {
                return None;
            }

            Some(NextDueSoonTimer {
                unit: timer.unit.clone(),
                next_run_utc: next_run_utc.clone(),
                time_until_next_sec,
                active_state: timer.active_state.clone(),
                trigger_unit: timer.trigger_unit.clone(),
            })
        })
        .collect::<Vec<_>>();
    next_due_soon.sort_by(|left, right| {
        left.time_until_next_sec
            .cmp(&right.time_until_next_sec)
            .then_with(|| left.unit.cmp(&right.unit))
    });
    next_due_soon.truncate(5);

    let mut failed_or_problem_timers = items
        .iter()
        .filter(|timer| {
            timer.overdue
                || timer.active_state.eq_ignore_ascii_case("failed")
                || timer.sub_state.eq_ignore_ascii_case("failed")
                || timer
                    .result
                    .as_deref()
                    .map(|result| !result.eq_ignore_ascii_case("success"))
                    .unwrap_or(false)
        })
        .map(|timer| ProblemTimer {
            unit: timer.unit.clone(),
            active_state: timer.active_state.clone(),
            sub_state: timer.sub_state.clone(),
            result: timer.result.clone(),
            overdue: timer.overdue,
            overdue_reason: timer.overdue_reason.clone(),
        })
        .collect::<Vec<_>>();
    failed_or_problem_timers.sort_by(|left, right| left.unit.cmp(&right.unit));

    TimerSummary {
        counts_by_active_state,
        overdue_count,
        next_due_soon,
        failed_or_problem_timers,
    }
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
        "list_timers" => {
            let query_params = match parse_timers_query_params(tool_call.arguments) {
                Ok(value) => value,
                Err(err) => return app_error_to_json_rpc(id, err),
            };

            let limit = match normalize_timers_limit(query_params.limit) {
                Ok(value) => value,
                Err(err) => return app_error_to_json_rpc(id, err),
            };
            let timer_state = match normalize_timer_state(query_params.state) {
                Ok(value) => value,
                Err(err) => return app_error_to_json_rpc(id, err),
            };
            let name_contains = normalize_name_contains(query_params.name_contains)
                .map(|value| value.to_ascii_lowercase());
            let summary_enabled = query_params.summary.unwrap_or(false);
            let include_persistent = query_params.include_persistent.unwrap_or(false);
            let overdue_only = query_params.overdue_only.unwrap_or(false);
            let sort = match normalize_timers_sort(query_params.sort) {
                Ok(value) => value,
                Err(err) => return app_error_to_json_rpc(id, err),
            };
            let order = match normalize_timers_order(query_params.order) {
                Ok(value) => value,
                Err(err) => return app_error_to_json_rpc(id, err),
            };

            match state.unit_provider.list_timer_units().await {
                Ok(timers) => {
                    let now = Utc::now();
                    let mut timers = timers
                        .into_iter()
                        .filter(|timer| {
                            timer_state
                                .as_deref()
                                .map(|expected| timer.active_state.eq_ignore_ascii_case(expected))
                                .unwrap_or(true)
                        })
                        .filter(|timer| {
                            name_contains
                                .as_deref()
                                .map(|needle| timer.unit.to_ascii_lowercase().contains(needle))
                                .unwrap_or(true)
                        })
                        .map(|timer| build_timer_item(timer, now, include_persistent))
                        .collect::<Vec<_>>();

                    if overdue_only {
                        timers.retain(|timer| timer.overdue);
                    }

                    sort_timer_items(&mut timers, &sort, &order);

                    if summary_enabled {
                        let summary = build_timer_summary(&timers);
                        let generated_at_utc =
                            Utc::now().to_rfc3339_opts(SecondsFormat::Millis, true);

                        return json_rpc_result(
                            id,
                            serde_json::to_value(CallToolResult {
                                content: vec![ContentBlock::from(TextContent::new(
                                    "Returned timer triage summary".to_string(),
                                    None,
                                    None,
                                ))],
                                is_error: None,
                                meta: None,
                                structured_content: Some(serde_json::Map::from_iter([
                                    ("summary".to_string(), json!(summary)),
                                    ("total_scanned".to_string(), json!(timers.len())),
                                    ("returned".to_string(), json!(timers.len())),
                                    ("truncated".to_string(), json!(false)),
                                    ("generated_at_utc".to_string(), json!(generated_at_utc)),
                                ])),
                            })
                            .expect("list_timers summary serialization"),
                        );
                    }

                    let total_scanned = timers.len();
                    let timers = timers.into_iter().take(limit).collect::<Vec<_>>();
                    let returned = timers.len();
                    let truncated = total_scanned > returned;
                    let generated_at_utc = Utc::now().to_rfc3339_opts(SecondsFormat::Millis, true);

                    json_rpc_result(
                        id,
                        serde_json::to_value(CallToolResult {
                            content: vec![ContentBlock::from(TextContent::new(
                                format!("Returned {returned} of {total_scanned} timers"),
                                None,
                                None,
                            ))],
                            is_error: None,
                            meta: None,
                            structured_content: Some(serde_json::Map::from_iter([
                                ("timers".to_string(), json!(timers)),
                                ("total_scanned".to_string(), json!(total_scanned)),
                                ("returned".to_string(), json!(returned)),
                                ("truncated".to_string(), json!(truncated)),
                                ("generated_at_utc".to_string(), json!(generated_at_utc)),
                            ])),
                        })
                        .expect("list_timers tool result serialization"),
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
