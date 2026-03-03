use serde::Serialize;
use serde_json::{json, Value};
use std::collections::BTreeMap;

use crate::domain::responses::{generated_at_utc_string, tool_success_response};
use crate::domain::utils::{
    filter_services_by_name_contains, filter_services_by_state, normalize_name_contains,
    normalize_service_state, normalize_services_limit, sort_services,
};
use crate::errors::AppError;
use crate::mcp::rpc::{app_error_to_json_rpc, json_rpc_invalid_params};
use crate::AppState;

use super::ServicesQueryParams;

#[derive(Debug)]
struct NormalizedServicesQuery {
    state_filter: Option<String>,
    name_contains_filter: Option<String>,
    limit: usize,
    summary_enabled: bool,
}

enum NormalizeServicesError {
    InvalidParams,
    Domain(AppError),
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

/// Builds `list_services` summary payload for triage mode.
///
/// Includes state counts, a capped failed-unit list, and an optional degraded hint.
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

/// Parses and normalizes `list_services` arguments into a typed execution query.
///
/// This consolidates schema parsing and domain validation so downstream handler
/// logic can execute against pre-validated values.
fn normalize_services_query(
    arguments: Option<serde_json::Map<String, Value>>,
) -> Result<NormalizedServicesQuery, NormalizeServicesError> {
    let query_params: ServicesQueryParams =
        serde_json::from_value(json!(arguments.unwrap_or_default()))
            .map_err(|_| NormalizeServicesError::InvalidParams)?;

    let state_filter =
        normalize_service_state(query_params.state).map_err(NormalizeServicesError::Domain)?;
    let name_contains_filter = normalize_name_contains(query_params.name_contains);
    let limit =
        normalize_services_limit(query_params.limit).map_err(NormalizeServicesError::Domain)?;
    let summary_enabled = query_params.summary.unwrap_or(false);

    Ok(NormalizedServicesQuery {
        state_filter,
        name_contains_filter,
        limit,
        summary_enabled,
    })
}

/// Handles `list_services` tool execution.
///
/// Parses tool arguments, validates filters/limits, and returns either detailed
/// service rows or summary triage output.
pub async fn handle_list_services(
    state: &AppState,
    id: Option<Value>,
    arguments: Option<serde_json::Map<String, Value>>,
) -> Value {
    let normalized = match normalize_services_query(arguments) {
        Ok(value) => value,
        Err(NormalizeServicesError::InvalidParams) => return json_rpc_invalid_params(id),
        Err(NormalizeServicesError::Domain(err)) => return app_error_to_json_rpc(id, err),
    };

    match state.unit_provider.list_service_units().await {
        Ok(mut services) => {
            services = filter_services_by_state(services, normalized.state_filter.as_deref());
            services = filter_services_by_name_contains(
                services,
                normalized.name_contains_filter.as_deref(),
            );

            let failed_first = normalized.state_filter.as_deref() == Some("failed");
            sort_services(&mut services, failed_first);

            if normalized.summary_enabled {
                let summary = build_service_summary(&services);
                let generated_at_utc = generated_at_utc_string();

                return tool_success_response(
                    id,
                    "Returned service triage summary".to_string(),
                    serde_json::Map::from_iter([
                        ("summary".to_string(), json!(summary)),
                        ("generated_at_utc".to_string(), json!(generated_at_utc)),
                    ]),
                );
            }

            let total = services.len();
            let services = services
                .into_iter()
                .take(normalized.limit)
                .collect::<Vec<_>>();
            let returned = services.len();
            let truncated = total > returned;
            let generated_at_utc = generated_at_utc_string();

            tool_success_response(
                id,
                format!("Returned {returned} of {total} services"),
                serde_json::Map::from_iter([
                    ("services".to_string(), json!(services)),
                    ("total".to_string(), json!(total)),
                    ("returned".to_string(), json!(returned)),
                    ("truncated".to_string(), json!(truncated)),
                    ("generated_at_utc".to_string(), json!(generated_at_utc)),
                ]),
            )
        }
        Err(err) => app_error_to_json_rpc(id, err),
    }
}
