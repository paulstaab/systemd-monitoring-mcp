//! Domain-specific shared validations and formatting utilities

use crate::{errors::AppError, systemd_client::UnitStatus};
use chrono::{DateTime, Utc};

pub const MAX_LOG_LIMIT: usize = 1_000;
pub const DEFAULT_LOG_LIMIT: usize = 100;
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

pub fn normalize_unit(unit: Option<String>) -> Result<Option<String>, AppError> {
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

pub fn normalize_unit_name_prefix(prefix: Option<String>) -> Result<Option<String>, AppError> {
    let Some(value) = prefix else {
        return Ok(None);
    };

    let normalized = value.trim();
    if normalized.is_empty() {
        return Err(AppError::bad_request(
            "invalid_unit_name_prefix",
            "unit_name_prefix must contain only alphanumeric characters, dashes, underscores, dots, @, and :",
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
            "invalid_unit_name_prefix",
            "unit_name_prefix must contain only alphanumeric characters, dashes, underscores, dots, @, and :",
        ));
    }

    Ok(Some(normalized.to_string()))
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

pub fn filter_services_by_state(services: Vec<UnitStatus>, state: Option<&str>) -> Vec<UnitStatus> {
    let Some(state) = state else {
        return services;
    };

    services
        .into_iter()
        .filter(|service| service.state.eq_ignore_ascii_case(state))
        .collect()
}

pub fn filter_services_by_unit_name_prefix(
    services: Vec<UnitStatus>,
    unit_name_prefix: Option<&str>,
) -> Vec<UnitStatus> {
    let Some(unit_name_prefix) = unit_name_prefix else {
        return services;
    };

    services
        .into_iter()
        .filter(|service| service.name.starts_with(unit_name_prefix))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::{
        filter_services_by_state, filter_services_by_unit_name_prefix, normalize_service_state,
        normalize_unit_name_prefix,
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
                name: "a.service".to_string(),
                state: "active".to_string(),
                description: None,
            },
            UnitStatus {
                name: "b.service".to_string(),
                state: "failed".to_string(),
                description: None,
            },
        ];

        let filtered = filter_services_by_state(services, Some("FaIlEd"));
        assert_eq!(filtered.len(), 1);
        assert_eq!(filtered[0].name, "b.service");
    }

    #[test]
    fn normalizes_unit_name_prefix() {
        let prefix =
            normalize_unit_name_prefix(Some("  sshd@prod ".to_string())).expect("valid prefix");
        assert_eq!(prefix.as_deref(), Some("sshd@prod"));
    }

    #[test]
    fn rejects_unit_name_prefix_with_disallowed_characters() {
        let prefix = normalize_unit_name_prefix(Some("sshd/prod".to_string()));
        let error = prefix.expect_err("expected invalid unit name prefix");
        assert!(error.to_string().contains("bad request"));
    }

    #[test]
    fn filters_services_by_unit_name_prefix() {
        let services = vec![
            UnitStatus {
                name: "a.service".to_string(),
                state: "active".to_string(),
                description: None,
            },
            UnitStatus {
                name: "b.service".to_string(),
                state: "failed".to_string(),
                description: None,
            },
        ];

        let filtered = filter_services_by_unit_name_prefix(services, Some("b."));
        assert_eq!(filtered.len(), 1);
        assert_eq!(filtered[0].name, "b.service");
    }
}
