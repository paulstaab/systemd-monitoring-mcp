//! Systemd D-Bus and Journald host integrations
//!
//! Provides the raw connection primitives into the host OS Systemd bindings over dbus.
//! Includes systemd unit representations, log querying semantics, and mocked providers.

use async_trait::async_trait;
use chrono::{DateTime, SecondsFormat, Utc};
use regex::Regex;
use serde::Serialize;
use std::collections::HashMap;
use systemd::{daemon, journal};
use thiserror::Error;
use tracing::warn;
use zbus::{zvariant::OwnedObjectPath, Connection, Proxy};

use crate::errors::AppError;

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct UnitStatus {
    pub unit: String,
    pub description: String,
    pub load_state: String,
    pub active_state: String,
    pub sub_state: String,
    pub unit_file_state: Option<String>,
    pub since_utc: Option<String>,
    pub main_pid: Option<u32>,
    pub exec_main_status: Option<i32>,
    pub result: Option<String>,
}

#[derive(Debug, Clone)]
pub struct LogQuery {
    pub priority: Option<String>,
    pub unit: Option<String>,
    pub exclude_units: Vec<String>,
    pub grep: Option<String>,
    pub order: LogOrder,
    pub start_utc: Option<DateTime<Utc>>,
    pub end_utc: Option<DateTime<Utc>>,
    pub limit: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LogOrder {
    Asc,
    Desc,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct LogQueryResult {
    pub entries: Vec<JournalLogEntry>,
    pub total_scanned: Option<usize>,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct JournalLogEntry {
    pub timestamp_utc: String,
    pub unit: Option<String>,
    pub priority: Option<String>,
    pub hostname: Option<String>,
    pub pid: Option<i32>,
    pub message: Option<String>,
    pub cursor: Option<String>,
}

#[derive(Debug, Clone)]
struct RawUnit {
    name: String,
    description: String,
    load_state: String,
    active_state: String,
    sub_state: String,
    unit_path: OwnedObjectPath,
}

#[derive(Debug, Clone, Default)]
struct ServiceDetails {
    unit_file_state: Option<String>,
    since_utc: Option<String>,
    main_pid: Option<u32>,
    exec_main_status: Option<i32>,
    result: Option<String>,
}

type ListUnitRecord = (
    String,
    String,
    String,
    String,
    String,
    String,
    OwnedObjectPath,
    u32,
    String,
    OwnedObjectPath,
);

#[derive(Debug, Error)]
pub enum SystemdAvailabilityError {
    #[error("systemd is not running (systemd daemon::booted returned false)")]
    NotBooted,
    #[error("failed to detect systemd boot state: {0}")]
    BootState(String),
    #[error("failed to connect to system dbus: {0}")]
    DbusConnect(String),
    #[error("failed to create systemd dbus proxy: {0}")]
    ProxyCreate(String),
    #[error("failed to query systemd manager: {0}")]
    ManagerQuery(String),
}

pub async fn ensure_systemd_available() -> Result<(), SystemdAvailabilityError> {
    let is_booted =
        daemon::booted().map_err(|err| SystemdAvailabilityError::BootState(err.to_string()))?;
    if !is_booted {
        return Err(SystemdAvailabilityError::NotBooted);
    }

    let connection = Connection::system()
        .await
        .map_err(|err| SystemdAvailabilityError::DbusConnect(err.to_string()))?;

    let proxy = Proxy::new(
        &connection,
        "org.freedesktop.systemd1",
        "/org/freedesktop/systemd1",
        "org.freedesktop.systemd1.Manager",
    )
    .await
    .map_err(|err| SystemdAvailabilityError::ProxyCreate(err.to_string()))?;

    let _: Vec<ListUnitRecord> = proxy
        .call("ListUnits", &())
        .await
        .map_err(|err| SystemdAvailabilityError::ManagerQuery(err.to_string()))?;

    Ok(())
}

#[async_trait]
pub trait UnitProvider: Send + Sync {
    async fn list_service_units(&self) -> Result<Vec<UnitStatus>, AppError>;
    async fn list_journal_logs(&self, query: &LogQuery) -> Result<LogQueryResult, AppError>;
}

#[derive(Debug, Default)]
pub struct DbusSystemdClient;

impl DbusSystemdClient {
    pub fn new() -> Self {
        Self
    }
}

#[async_trait]
impl UnitProvider for DbusSystemdClient {
    async fn list_service_units(&self) -> Result<Vec<UnitStatus>, AppError> {
        let connection = Connection::system().await.map_err(|err| {
            AppError::internal(format!("failed to connect to system dbus: {err}"))
        })?;

        let proxy = Proxy::new(
            &connection,
            "org.freedesktop.systemd1",
            "/org/freedesktop/systemd1",
            "org.freedesktop.systemd1.Manager",
        )
        .await
        .map_err(|err| AppError::internal(format!("failed to create systemd dbus proxy: {err}")))?;

        let rows: Vec<ListUnitRecord> = proxy.call("ListUnits", &()).await.map_err(|err| {
            AppError::internal(format!("failed to list units from systemd: {err}"))
        })?;

        let raw_units: Vec<RawUnit> = rows
            .into_iter()
            .map(
                |(
                    name,
                    description,
                    load_state,
                    active_state,
                    sub_state,
                    _following,
                    unit_path,
                    _job_id,
                    _job_type,
                    _job_path,
                )| {
                    RawUnit {
                        name,
                        description,
                        load_state,
                        active_state,
                        sub_state,
                        unit_path,
                    }
                },
            )
            .collect();

        let mut units = map_and_sort_service_units(raw_units.clone());
        let unit_paths: HashMap<String, OwnedObjectPath> = raw_units
            .into_iter()
            .filter(|unit| unit.name.ends_with(".service"))
            .map(|unit| (unit.name, unit.unit_path))
            .collect();

        for unit in &mut units {
            let Some(unit_path) = unit_paths.get(&unit.unit) else {
                continue;
            };

            match fetch_service_details(&connection, unit_path).await {
                Ok(details) => {
                    unit.unit_file_state = details.unit_file_state;
                    unit.since_utc = details.since_utc;
                    unit.main_pid = details.main_pid;
                    unit.exec_main_status = details.exec_main_status;
                    unit.result = details.result;
                }
                Err(err) => {
                    warn!(
                        unit = %unit.unit,
                        unit_path = %unit_path.as_str(),
                        error = %err,
                        "failed to enrich service details from systemd"
                    );
                }
            }
        }

        Ok(units)
    }

    async fn list_journal_logs(&self, query: &LogQuery) -> Result<LogQueryResult, AppError> {
        let query = query.clone();
        tokio::task::spawn_blocking(move || read_journal_logs(&query))
            .await
            .map_err(|err| {
                AppError::internal(format!("failed to spawn journald reader task: {err}"))
            })?
    }
}

fn map_and_sort_service_units(raw_units: Vec<RawUnit>) -> Vec<UnitStatus> {
    let mut units: Vec<UnitStatus> = raw_units
        .into_iter()
        .filter(|unit| unit.name.ends_with(".service"))
        .map(|unit| UnitStatus {
            unit: unit.name,
            description: unit.description,
            load_state: unit.load_state,
            active_state: unit.active_state,
            sub_state: unit.sub_state,
            unit_file_state: None,
            since_utc: None,
            main_pid: None,
            exec_main_status: None,
            result: None,
        })
        .collect();

    units.sort_by(|left, right| left.unit.cmp(&right.unit));
    units
}

async fn fetch_service_details(
    connection: &Connection,
    unit_path: &OwnedObjectPath,
) -> Result<ServiceDetails, AppError> {
    let unit_proxy = Proxy::new(
        connection,
        "org.freedesktop.systemd1",
        unit_path,
        "org.freedesktop.systemd1.Unit",
    )
    .await
    .map_err(|err| {
        AppError::internal(format!(
            "failed to create systemd unit proxy for {}: {err}",
            unit_path.as_str()
        ))
    })?;

    let unit_file_state = try_get_string_property(&unit_proxy, "UnitFileState").await?;
    let since_utc = try_get_u64_property(&unit_proxy, "ActiveEnterTimestamp")
        .await?
        .and_then(format_systemd_timestamp_usec);

    let service_proxy = Proxy::new(
        connection,
        "org.freedesktop.systemd1",
        unit_path,
        "org.freedesktop.systemd1.Service",
    )
    .await
    .map_err(|err| {
        AppError::internal(format!(
            "failed to create systemd service proxy for {}: {err}",
            unit_path.as_str()
        ))
    })?;

    let main_pid = try_get_u32_property(&service_proxy, "MainPID")
        .await?
        .filter(|value| *value > 0);

    let exec_main_status = try_get_u32_property(&service_proxy, "ExecMainStatus")
        .await?
        .and_then(|value| i32::try_from(value).ok());

    let result = try_get_string_property(&service_proxy, "Result").await?;

    Ok(ServiceDetails {
        unit_file_state,
        since_utc,
        main_pid,
        exec_main_status,
        result,
    })
}

async fn try_get_string_property(
    proxy: &Proxy<'_>,
    property_name: &str,
) -> Result<Option<String>, AppError> {
    proxy
        .get_property::<String>(property_name)
        .await
        .map(|value| {
            if value.trim().is_empty() {
                None
            } else {
                Some(value)
            }
        })
        .map_err(|err| {
            AppError::internal(format!(
                "failed to read systemd property {property_name}: {err}"
            ))
        })
}

async fn try_get_u64_property(
    proxy: &Proxy<'_>,
    property_name: &str,
) -> Result<Option<u64>, AppError> {
    proxy
        .get_property::<u64>(property_name)
        .await
        .map(Some)
        .map_err(|err| {
            AppError::internal(format!(
                "failed to read systemd property {property_name}: {err}"
            ))
        })
}

async fn try_get_u32_property(
    proxy: &Proxy<'_>,
    property_name: &str,
) -> Result<Option<u32>, AppError> {
    proxy
        .get_property::<u32>(property_name)
        .await
        .map(Some)
        .map_err(|err| {
            AppError::internal(format!(
                "failed to read systemd property {property_name}: {err}"
            ))
        })
}

fn format_systemd_timestamp_usec(timestamp_usec: u64) -> Option<String> {
    if timestamp_usec == 0 {
        return None;
    }

    i64::try_from(timestamp_usec)
        .ok()
        .and_then(DateTime::<Utc>::from_timestamp_micros)
        .map(|timestamp| timestamp.to_rfc3339_opts(SecondsFormat::Millis, true))
}

#[derive(Debug)]
enum GrepMatcher {
    Substring(String),
    Regex(Regex),
}

fn build_grep_matcher(grep: Option<&str>) -> Result<Option<GrepMatcher>, AppError> {
    let Some(grep) = grep else {
        return Ok(None);
    };

    let trimmed = grep.trim();
    if trimmed.is_empty() {
        return Ok(None);
    }

    if trimmed.len() >= 2 && trimmed.starts_with('/') && trimmed.ends_with('/') {
        let pattern = &trimmed[1..trimmed.len() - 1];
        let regex = Regex::new(pattern)
            .map_err(|_| AppError::bad_request("invalid_grep", "grep regex pattern is invalid"))?;
        return Ok(Some(GrepMatcher::Regex(regex)));
    }

    Ok(Some(GrepMatcher::Substring(trimmed.to_string())))
}

fn matches_grep(matcher: &Option<GrepMatcher>, message: &str) -> bool {
    let Some(matcher) = matcher else {
        return true;
    };

    match matcher {
        GrepMatcher::Substring(value) => message.contains(value),
        GrepMatcher::Regex(regex) => regex.is_match(message),
    }
}

fn sanitize_log_message(message: Option<String>) -> Option<String> {
    message.and_then(|value| {
        let trimmed = value.trim();
        if trimmed.is_empty() {
            return None;
        }

        let sanitized: String = trimmed
            .chars()
            .map(|character| {
                if character.is_control()
                    && character != '\n'
                    && character != '\r'
                    && character != '\t'
                {
                    ' '
                } else {
                    character
                }
            })
            .collect();

        let sanitized_trimmed = sanitized.trim();
        if sanitized_trimmed.is_empty() {
            None
        } else {
            Some(sanitized_trimmed.to_string())
        }
    })
}

fn read_journal_logs(query: &LogQuery) -> Result<LogQueryResult, AppError> {
    let mut reader = journal::OpenOptions::default()
        .open()
        .map_err(|err| AppError::internal(format!("failed to open journald reader: {err}")))?;

    let grep_matcher = build_grep_matcher(query.grep.as_deref())?;

    if let Some(unit) = &query.unit {
        reader
            .match_add("_SYSTEMD_UNIT", unit.as_bytes())
            .map_err(|err| AppError::internal(format!("failed to apply unit filter: {err}")))?;
    }

    let Some(start_utc) = query.start_utc else {
        return Err(AppError::bad_request("start_utc must be set".to_string()));
    };
    let Some(end_utc) = query.end_utc else {
        return Err(AppError::bad_request("end_utc must be set".to_string()));
    };

    match query.order {
        LogOrder::Desc => {
            let end_unix_usec = end_utc.timestamp_micros();
            if let Ok(end_unix_usec) = u64::try_from(end_unix_usec) {
                reader.seek_realtime_usec(end_unix_usec).map_err(|err| {
                    AppError::internal(format!("failed to seek journald end timestamp: {err}"))
                })?;
            } else {
                reader.seek_tail().map_err(|err| {
                    AppError::internal(format!("failed to seek journald tail: {err}"))
                })?;
            }
        }
        LogOrder::Asc => {
            let start_unix_usec = start_utc.timestamp_micros();
            if let Ok(start_unix_usec) = u64::try_from(start_unix_usec) {
                reader.seek_realtime_usec(start_unix_usec).map_err(|err| {
                    AppError::internal(format!("failed to seek journald start timestamp: {err}"))
                })?;
            } else {
                reader.seek_head().map_err(|err| {
                    AppError::internal(format!("failed to seek journald head: {err}"))
                })?;
            }
        }
    }

    let threshold = query
        .priority
        .as_deref()
        .and_then(|value| value.parse::<u8>().ok());
    let start_unix_usec = start_utc.timestamp_micros();
    let end_unix_usec = end_utc.timestamp_micros();

    let mut entries = Vec::new();
    let mut total_scanned = 0usize;

    loop {
        if entries.len() >= query.limit {
            break;
        }

        let advanced = match query.order {
            LogOrder::Desc => reader.previous(),
            LogOrder::Asc => reader.next(),
        }
        .map_err(|err| AppError::internal(format!("failed to read journald entry: {err}")))?;

        if advanced == 0 {
            break;
        }
        total_scanned += 1;

        let timestamp_unix_usec_u64 = reader.timestamp_usec().map_err(|err| {
            AppError::internal(format!("failed to read journald timestamp: {err}"))
        })?;
        let Ok(timestamp_unix_usec) = i64::try_from(timestamp_unix_usec_u64) else {
            continue;
        };

        if timestamp_unix_usec < start_unix_usec {
            if query.order == LogOrder::Desc {
                break;
            }
            continue;
        }

        if timestamp_unix_usec > end_unix_usec {
            if query.order == LogOrder::Asc {
                break;
            }
            continue;
        }

        let unit = read_journal_field(&mut reader, "_SYSTEMD_UNIT")?;
        if let Some(unit) = unit.as_deref() {
            if query
                .exclude_units
                .iter()
                .any(|excluded| excluded.eq_ignore_ascii_case(unit))
            {
                continue;
            }
        }

        let Some(timestamp) = DateTime::<Utc>::from_timestamp_micros(timestamp_unix_usec) else {
            continue;
        };

        let priority =
            read_journal_field(&mut reader, "PRIORITY")?.and_then(|value| value.parse::<u8>().ok());

        if let Some(max_priority) = threshold {
            match priority {
                Some(entry_priority) if entry_priority <= max_priority => {}
                _ => continue,
            }
        }

        let timestamp_utc = timestamp.to_rfc3339_opts(SecondsFormat::Millis, true);
        let hostname = read_journal_field(&mut reader, "_HOSTNAME")?;
        let pid =
            read_journal_field(&mut reader, "_PID")?.and_then(|value| value.parse::<i32>().ok());
        let message = sanitize_log_message(read_journal_field(&mut reader, "MESSAGE")?);
        if let Some(message) = message.as_deref() {
            if !matches_grep(&grep_matcher, message) {
                continue;
            }
        } else if query.grep.is_some() {
            continue;
        }
        let cursor = reader.cursor().ok();

        entries.push(JournalLogEntry {
            timestamp_utc,
            unit,
            priority: priority.map(|value| value.to_string()),
            hostname,
            pid,
            message,
            cursor,
        });
    }

    Ok(LogQueryResult {
        entries,
        total_scanned: Some(total_scanned),
    })
}

fn read_journal_field(
    reader: &mut systemd::Journal,
    field: &str,
) -> Result<Option<String>, AppError> {
    let data = reader.get_data(field).map_err(|err| {
        AppError::internal(format!("failed to read journald field {field}: {err}"))
    })?;

    let Some(data) = data else {
        return Ok(None);
    };

    let Some(value) = data.value() else {
        return Ok(None);
    };

    // Use a lossy UTF-8 conversion so non-UTF8 bytes don't cause the field
    // to be dropped. This preserves the presence of the field while replacing
    // invalid sequences with the Unicode replacement character.
    Ok(Some(String::from_utf8_lossy(value).into_owned()))
}

#[cfg(test)]
mod tests {
    use super::{map_and_sort_service_units, JournalLogEntry, RawUnit};
    use zbus::zvariant::OwnedObjectPath;

    #[test]
    fn filters_non_service_and_sorts() {
        let mapped = map_and_sort_service_units(vec![
            RawUnit {
                name: "z.service".to_string(),
                description: "".to_string(),
                load_state: "loaded".to_string(),
                active_state: "active".to_string(),
                sub_state: "running".to_string(),
                unit_path: OwnedObjectPath::try_from("/org/freedesktop/systemd1/unit/z_2eservice")
                    .expect("valid object path"),
            },
            RawUnit {
                name: "a.socket".to_string(),
                description: "Socket".to_string(),
                load_state: "loaded".to_string(),
                active_state: "active".to_string(),
                sub_state: "running".to_string(),
                unit_path: OwnedObjectPath::try_from("/org/freedesktop/systemd1/unit/a_2esocket")
                    .expect("valid object path"),
            },
            RawUnit {
                name: "a.service".to_string(),
                description: "Alpha".to_string(),
                load_state: "loaded".to_string(),
                active_state: "failed".to_string(),
                sub_state: "failed".to_string(),
                unit_path: OwnedObjectPath::try_from("/org/freedesktop/systemd1/unit/a_2eservice")
                    .expect("valid object path"),
            },
        ]);

        assert_eq!(mapped.len(), 2);
        assert_eq!(mapped[0].unit, "a.service");
        assert_eq!(mapped[0].description, "Alpha");
        assert_eq!(mapped[0].load_state, "loaded");
        assert_eq!(mapped[0].active_state, "failed");
        assert_eq!(mapped[0].sub_state, "failed");
        assert_eq!(mapped[1].unit, "z.service");
        assert_eq!(mapped[1].description, "");
    }

    #[test]
    fn journal_log_entry_keeps_expected_shape() {
        let sample = JournalLogEntry {
            timestamp_utc: "2025-01-01T00:00:00.000Z".to_string(),
            unit: Some("ssh.service".to_string()),
            priority: Some("6".to_string()),
            hostname: Some("host-a".to_string()),
            pid: Some(1234),
            message: Some("Started OpenSSH server".to_string()),
            cursor: Some("s=abc;i=12".to_string()),
        };

        assert_eq!(sample.unit.as_deref(), Some("ssh.service"));
        assert_eq!(sample.priority.as_deref(), Some("6"));
    }
}
