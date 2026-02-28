//! Domain-specific shared validations and formatting utilities

use crate::{errors::AppError, systemd_client::UnitStatus};
use chrono::{DateTime, Utc};

pub const MAX_LOG_LIMIT: usize = 1_000;
pub const DEFAULT_LOG_LIMIT: usize = 100;
pub const MAX_SERVICES_LIMIT: usize = 1_000;
pub const DEFAULT_SERVICES_LIMIT: usize = 200;
pub const VALID_SERVICE_STATES: [&str; 6] = [
    "active",
    "inactive",
    "failed",
    "activating",
    "deactivating",
    "reloading",
];

pub fn parse_utc(value: &Option<String>) -> Result<Option<DateTime<Utc>>, AppError> {
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

pub fn normalize_priority(priority: Option<String>) -> Result<Option<String>, AppError> {
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

fn is_valid_unit_name_chars(s: &str) -> bool {
    s.chars().all(|character| {
        character.is_ascii_alphanumeric()
            || character == '-'
            || character == '_'
            || character == '@'
            || character == ':'
            || character == '.'
    })
}

pub fn normalize_unit(unit: Option<String>) -> Result<Option<String>, AppError> {
    let Some(value) = unit else {
        return Ok(None);
    };

    let normalized = value.trim();
    if normalized.is_empty() || !is_valid_unit_name_chars(normalized) {
        return Err(AppError::bad_request(
            "invalid_unit",
            "unit must contain only alphanumeric characters, dashes, underscores, dots, @, and :",
        ));
    }

    Ok(Some(normalized.to_string()))
}

pub fn normalize_name_contains(value: Option<String>) -> Option<String> {
    let value = value?;

    let normalized = value.trim();
    if normalized.is_empty() {
        return None;
    }

    Some(normalized.to_string())
}

pub fn normalize_service_state(state: Option<String>) -> Result<Option<String>, AppError> {
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

pub fn normalize_services_limit(limit: Option<u32>) -> Result<usize, AppError> {
    let limit = limit.unwrap_or(DEFAULT_SERVICES_LIMIT as u32);
    if limit == 0 || limit > MAX_SERVICES_LIMIT as u32 {
        return Err(AppError::bad_request(
            "invalid_limit",
            "limit must be between 1 and 1000",
        ));
    }

    Ok(limit as usize)
}

pub fn filter_services_by_state(services: Vec<UnitStatus>, state: Option<&str>) -> Vec<UnitStatus> {
    let Some(state) = state else {
        return services;
    };

    services
        .into_iter()
        .filter(|service| service.active_state.eq_ignore_ascii_case(state))
        .collect()
}

pub fn filter_services_by_name_contains(
    services: Vec<UnitStatus>,
    name_contains: Option<&str>,
) -> Vec<UnitStatus> {
    let Some(name_contains) = name_contains else {
        return services;
    };

    services
        .into_iter()
        .filter(|service| service.unit.contains(name_contains))
        .collect()
}

pub fn sort_services(services: &mut [UnitStatus], failed_first: bool) {
    if failed_first {
        services.sort_by(|left, right| {
            let left_failed = left.active_state.eq_ignore_ascii_case("failed");
            let right_failed = right.active_state.eq_ignore_ascii_case("failed");

            right_failed
                .cmp(&left_failed)
                .then_with(|| left.unit.cmp(&right.unit))
        });
        return;
    }

    services.sort_by(|left, right| left.unit.cmp(&right.unit));
}

#[cfg(test)]
mod tests {
    use super::{
        filter_services_by_name_contains, filter_services_by_state, normalize_name_contains,
        normalize_service_state, normalize_services_limit, sort_services,
    };
    use crate::systemd_client::UnitStatus;

    #[test]
    fn normalizes_service_state_test() {
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
                unit: "a.service".to_string(),
                description: "A".to_string(),
                load_state: "loaded".to_string(),
                active_state: "active".to_string(),
                sub_state: "running".to_string(),
                unit_file_state: None,
                since_utc: None,
                main_pid: None,
                exec_main_status: None,
                result: None,
            },
            UnitStatus {
                unit: "b.service".to_string(),
                description: "B".to_string(),
                load_state: "loaded".to_string(),
                active_state: "failed".to_string(),
                sub_state: "failed".to_string(),
                unit_file_state: None,
                since_utc: None,
                main_pid: None,
                exec_main_status: None,
                result: None,
            },
        ];

        let filtered = filter_services_by_state(services, Some("FaIlEd"));
        assert_eq!(filtered.len(), 1);
        assert_eq!(filtered[0].unit, "b.service");
    }

    #[test]
    fn normalizes_name_contains() {
        let filter = normalize_name_contains(Some("  sshd@prod ".to_string()));
        assert_eq!(filter.as_deref(), Some("sshd@prod"));
    }

    #[test]
    fn empty_name_contains_treated_as_none() {
        let filter = normalize_name_contains(Some("   ".to_string()));
        assert_eq!(filter, None);
    }

    #[test]
    fn rejects_invalid_services_limit() {
        let error = normalize_services_limit(Some(1_001)).expect_err("invalid limit");
        assert!(error.to_string().contains("bad request"));
    }

    #[test]
    fn filters_services_by_name_contains() {
        let services = vec![
            UnitStatus {
                unit: "a.service".to_string(),
                description: "A".to_string(),
                load_state: "loaded".to_string(),
                active_state: "active".to_string(),
                sub_state: "running".to_string(),
                unit_file_state: None,
                since_utc: None,
                main_pid: None,
                exec_main_status: None,
                result: None,
            },
            UnitStatus {
                unit: "b.service".to_string(),
                description: "B".to_string(),
                load_state: "loaded".to_string(),
                active_state: "failed".to_string(),
                sub_state: "failed".to_string(),
                unit_file_state: None,
                since_utc: None,
                main_pid: None,
                exec_main_status: None,
                result: None,
            },
        ];

        let filtered = filter_services_by_name_contains(services, Some("b."));
        assert_eq!(filtered.len(), 1);
        assert_eq!(filtered[0].unit, "b.service");
    }

    #[test]
    fn sorts_failed_first_then_unit() {
        let mut services = vec![
            UnitStatus {
                unit: "z.service".to_string(),
                description: "Z".to_string(),
                load_state: "loaded".to_string(),
                active_state: "active".to_string(),
                sub_state: "running".to_string(),
                unit_file_state: None,
                since_utc: None,
                main_pid: None,
                exec_main_status: None,
                result: None,
            },
            UnitStatus {
                unit: "a.service".to_string(),
                description: "A".to_string(),
                load_state: "loaded".to_string(),
                active_state: "failed".to_string(),
                sub_state: "failed".to_string(),
                unit_file_state: None,
                since_utc: None,
                main_pid: None,
                exec_main_status: None,
                result: None,
            },
        ];

        sort_services(&mut services, true);
        assert_eq!(services[0].unit, "a.service");
    }
}
