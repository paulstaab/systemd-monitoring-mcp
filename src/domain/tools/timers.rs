use chrono::{DateTime, Duration, Utc};
use serde::Serialize;
use serde_json::{json, Value};
use std::cmp::Ordering;
use std::collections::BTreeMap;

use crate::domain::responses::{generated_at_utc_string, tool_success_response};
use crate::domain::utils::{
    normalize_name_contains, normalize_scope, normalize_timer_state, normalize_timers_limit,
    normalize_timers_order, normalize_timers_sort,
};
use crate::mcp::rpc::app_error_to_json_rpc;
use crate::systemd_client::UnitScope;
use crate::{errors::AppError, AppState};

#[derive(Debug)]
pub struct TimersQueryParams {
    pub scope: Option<String>,
    pub limit: Option<u32>,
    pub name_contains: Option<String>,
    pub state: Option<String>,
    pub summary: Option<bool>,
    pub include_persistent: Option<bool>,
    pub overdue_only: Option<bool>,
    pub sort: Option<String>,
    pub order: Option<String>,
}

#[derive(Debug)]
struct NormalizedTimersQuery {
    scope: UnitScope,
    limit: usize,
    timer_state: Option<String>,
    name_contains: Option<String>,
    summary_enabled: bool,
    include_persistent: bool,
    overdue_only: bool,
    sort: String,
    order: String,
}

#[derive(Debug, Serialize, Clone)]
pub struct TimerItem {
    pub unit: String,
    pub active_state: String,
    pub sub_state: String,
    pub next_run_utc: Option<String>,
    pub last_run_utc: Option<String>,
    pub time_until_next_sec: Option<i64>,
    pub time_since_last_sec: Option<i64>,
    pub trigger_unit: Option<String>,
    pub persistent: Option<bool>,
    pub result: Option<String>,
    pub load_state: Option<String>,
    pub unit_file_state: Option<String>,
    pub overdue: bool,
    pub overdue_reason: Option<String>,
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

/// Extracts an optional boolean argument from a tool-argument map.
///
/// Returns `Ok(None)` when the key is missing.
/// Returns `invalid_params` when the key exists but is not a JSON boolean.
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
pub fn parse_timers_query_params(
    arguments: Option<serde_json::Map<String, Value>>,
) -> Result<TimersQueryParams, AppError> {
    let arguments = arguments.unwrap_or_default();

    Ok(TimersQueryParams {
        scope: parse_optional_string_argument(&arguments, "scope")?,
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

/// Parses and normalizes `list_timers` arguments into a typed execution query.
///
/// This function centralizes strict argument parsing and domain-specific
/// normalization so the runtime handler operates only on validated values.
fn normalize_timers_query(
    arguments: Option<serde_json::Map<String, Value>>,
) -> Result<NormalizedTimersQuery, AppError> {
    let query_params = parse_timers_query_params(arguments)?;
    let scope = normalize_scope(query_params.scope)?;
    let limit = normalize_timers_limit(query_params.limit)?;
    let timer_state = normalize_timer_state(query_params.state)?;
    let name_contains =
        normalize_name_contains(query_params.name_contains).map(|value| value.to_ascii_lowercase());
    let summary_enabled = query_params.summary.unwrap_or(false);
    let include_persistent = query_params.include_persistent.unwrap_or(false);
    let overdue_only = query_params.overdue_only.unwrap_or(false);
    let sort = normalize_timers_sort(query_params.sort)?;
    let order = normalize_timers_order(query_params.order)?;

    Ok(NormalizedTimersQuery {
        scope,
        limit,
        timer_state,
        name_contains,
        summary_enabled,
        include_persistent,
        overdue_only,
        sort,
        order,
    })
}

/// Parses an optional RFC3339 timestamp string to UTC.
///
/// Returns `None` for absent or unparsable values. This is used for best-effort
/// enrichment on timer metadata where partial data is acceptable.
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
pub fn sort_timer_items(items: &mut [TimerItem], sort: &str, order: &str) {
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

/// Handles `list_timers` tool execution.
///
/// Parses and validates tool arguments, transforms timer metadata into API-facing
/// rows, and returns either detailed rows or summary triage output.
pub async fn handle_list_timers(
    state: &AppState,
    id: Option<Value>,
    arguments: Option<serde_json::Map<String, Value>>,
) -> Value {
    let normalized = match normalize_timers_query(arguments) {
        Ok(value) => value,
        Err(err) => return app_error_to_json_rpc(id, err),
    };

    match state.unit_provider.list_timer_units(normalized.scope).await {
        Ok(timers) => {
            let now = Utc::now();
            let mut timers = timers
                .into_iter()
                .filter(|timer| {
                    normalized
                        .timer_state
                        .as_deref()
                        .map(|expected| timer.active_state.eq_ignore_ascii_case(expected))
                        .unwrap_or(true)
                })
                .filter(|timer| {
                    normalized
                        .name_contains
                        .as_deref()
                        .map(|needle| timer.unit.to_ascii_lowercase().contains(needle))
                        .unwrap_or(true)
                })
                .map(|timer| build_timer_item(timer, now, normalized.include_persistent))
                .collect::<Vec<_>>();

            if normalized.overdue_only {
                timers.retain(|timer| timer.overdue);
            }

            sort_timer_items(&mut timers, &normalized.sort, &normalized.order);

            if normalized.summary_enabled {
                let summary = build_timer_summary(&timers);
                let generated_at_utc = generated_at_utc_string();

                return tool_success_response(
                    id,
                    "Returned timer triage summary".to_string(),
                    serde_json::Map::from_iter([
                        ("summary".to_string(), json!(summary)),
                        ("total_scanned".to_string(), json!(timers.len())),
                        ("returned".to_string(), json!(timers.len())),
                        ("truncated".to_string(), json!(false)),
                        ("generated_at_utc".to_string(), json!(generated_at_utc)),
                    ]),
                );
            }

            let total_scanned = timers.len();
            let timers = timers
                .into_iter()
                .take(normalized.limit)
                .collect::<Vec<_>>();
            let returned = timers.len();
            let truncated = total_scanned > returned;
            let generated_at_utc = generated_at_utc_string();

            tool_success_response(
                id,
                format!("Returned {returned} of {total_scanned} timers"),
                serde_json::Map::from_iter([
                    ("timers".to_string(), json!(timers)),
                    ("total_scanned".to_string(), json!(total_scanned)),
                    ("returned".to_string(), json!(returned)),
                    ("truncated".to_string(), json!(truncated)),
                    ("generated_at_utc".to_string(), json!(generated_at_utc)),
                ]),
            )
        }
        Err(err) => app_error_to_json_rpc(id, err),
    }
}
