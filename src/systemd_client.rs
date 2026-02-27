use async_trait::async_trait;
use chrono::{DateTime, SecondsFormat, Utc};
use serde::Serialize;
use serde_json::Value;
use std::process::Command;
use thiserror::Error;
use zbus::{zvariant::OwnedObjectPath, Connection, Proxy};

use crate::errors::AppError;

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct UnitStatus {
    pub name: String,
    pub state: String,
    pub description: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LogSortOrder {
    Asc,
    Desc,
}

#[derive(Debug, Clone)]
pub struct LogQuery {
    pub priority: Option<String>,
    pub unit: Option<String>,
    pub start_utc: Option<DateTime<Utc>>,
    pub end_utc: Option<DateTime<Utc>>,
    pub limit: usize,
    pub order: LogSortOrder,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct JournalLogEntry {
    pub timestamp_utc: String,
    pub timestamp_unix_usec: i64,
    pub unit: Option<String>,
    pub priority: Option<u8>,
    pub message: Option<String>,
}

#[derive(Debug, Clone)]
struct RawUnit {
    name: String,
    description: String,
    active_state: String,
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
    #[error("systemd is not running (libsystemd daemon::booted returned false)")]
    NotBooted,
    #[error("failed to connect to system dbus: {0}")]
    DbusConnect(String),
    #[error("failed to create systemd dbus proxy: {0}")]
    ProxyCreate(String),
    #[error("failed to query systemd manager: {0}")]
    ManagerQuery(String),
}

pub async fn ensure_systemd_available() -> Result<(), SystemdAvailabilityError> {
    if !libsystemd::daemon::booted() {
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
    async fn list_journal_logs(&self, query: &LogQuery) -> Result<Vec<JournalLogEntry>, AppError>;
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
                    _load_state,
                    active_state,
                    _sub_state,
                    _following,
                    _unit_path,
                    _job_id,
                    _job_type,
                    _job_path,
                )| {
                    RawUnit {
                        name,
                        description,
                        active_state,
                    }
                },
            )
            .collect();

        Ok(map_and_sort_service_units(raw_units))
    }

    async fn list_journal_logs(&self, query: &LogQuery) -> Result<Vec<JournalLogEntry>, AppError> {
        let mut command = Command::new("journalctl");
        command.arg("--output=json").arg("--no-pager").arg("--utc");

        if let Some(priority) = &query.priority {
            command.arg(format!("--priority={priority}"));
        }

        if let Some(unit) = &query.unit {
            command.arg(format!("--unit={unit}"));
        }

        if let Some(start_utc) = query.start_utc {
            command.arg(format!("--since={}", start_utc.to_rfc3339()));
        }

        if let Some(end_utc) = query.end_utc {
            command.arg(format!("--until={}", end_utc.to_rfc3339()));
        }

        let output = command
            .output()
            .map_err(|err| AppError::internal(format!("failed to execute journalctl: {err}")))?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(AppError::internal(format!(
                "journalctl command failed: {}",
                stderr.trim()
            )));
        }

        let stdout = String::from_utf8_lossy(&output.stdout);
        let mut entries = parse_journal_output(&stdout)?;

        entries.sort_by_key(|entry| entry.timestamp_unix_usec);
        if matches!(query.order, LogSortOrder::Desc) {
            entries.reverse();
        }

        if entries.len() > query.limit {
            entries.truncate(query.limit);
        }

        Ok(entries)
    }
}

fn map_and_sort_service_units(raw_units: Vec<RawUnit>) -> Vec<UnitStatus> {
    let mut units: Vec<UnitStatus> = raw_units
        .into_iter()
        .filter(|unit| unit.name.ends_with(".service"))
        .map(|unit| UnitStatus {
            name: unit.name,
            state: unit.active_state,
            description: if unit.description.trim().is_empty() {
                None
            } else {
                Some(unit.description)
            },
        })
        .collect();

    units.sort_by(|left, right| left.name.cmp(&right.name));
    units
}

fn parse_journal_output(output: &str) -> Result<Vec<JournalLogEntry>, AppError> {
    let mut entries = Vec::new();

    for line in output.lines().filter(|line| !line.trim().is_empty()) {
        let payload: Value = serde_json::from_str(line).map_err(|err| {
            AppError::internal(format!("failed to parse journalctl JSON output: {err}"))
        })?;

        let timestamp_unix_usec = payload
            .get("__REALTIME_TIMESTAMP")
            .and_then(Value::as_str)
            .and_then(|value| value.parse::<i64>().ok())
            .ok_or_else(|| {
                AppError::internal(
                    "journal entry is missing __REALTIME_TIMESTAMP or has invalid format",
                )
            })?;

        let timestamp_utc = DateTime::<Utc>::from_timestamp_micros(timestamp_unix_usec)
            .ok_or_else(|| AppError::internal("journal entry timestamp is out of supported range"))?
            .to_rfc3339_opts(SecondsFormat::Millis, true);

        let unit = payload
            .get("_SYSTEMD_UNIT")
            .and_then(Value::as_str)
            .map(ToString::to_string);

        let priority = payload
            .get("PRIORITY")
            .and_then(Value::as_str)
            .and_then(|value| value.parse::<u8>().ok());

        let message = payload
            .get("MESSAGE")
            .and_then(Value::as_str)
            .map(ToString::to_string);

        entries.push(JournalLogEntry {
            timestamp_utc,
            timestamp_unix_usec,
            unit,
            priority,
            message,
        });
    }

    Ok(entries)
}

#[cfg(test)]
mod tests {
    use super::{map_and_sort_service_units, parse_journal_output, JournalLogEntry, RawUnit};

    #[test]
    fn filters_non_service_and_sorts() {
        let mapped = map_and_sort_service_units(vec![
            RawUnit {
                name: "z.service".to_string(),
                description: "".to_string(),
                active_state: "active".to_string(),
            },
            RawUnit {
                name: "a.socket".to_string(),
                description: "Socket".to_string(),
                active_state: "active".to_string(),
            },
            RawUnit {
                name: "a.service".to_string(),
                description: "Alpha".to_string(),
                active_state: "failed".to_string(),
            },
        ]);

        assert_eq!(mapped.len(), 2);
        assert_eq!(mapped[0].name, "a.service");
        assert_eq!(mapped[0].description.as_deref(), Some("Alpha"));
        assert_eq!(mapped[1].name, "z.service");
        assert!(mapped[1].description.is_none());
    }

    #[test]
    fn parses_journal_json_lines() {
        let raw = r#"{"__REALTIME_TIMESTAMP":"1735689600000000","_SYSTEMD_UNIT":"ssh.service","PRIORITY":"6","MESSAGE":"Started OpenSSH server"}
{"__REALTIME_TIMESTAMP":"1735689601000000","PRIORITY":"3","MESSAGE":"Service failed"}"#;

        let parsed = parse_journal_output(raw).expect("journal output should parse");

        assert_eq!(parsed.len(), 2);
        assert_eq!(
            parsed[0],
            JournalLogEntry {
                timestamp_utc: "2025-01-01T00:00:00.000Z".to_string(),
                timestamp_unix_usec: 1_735_689_600_000_000,
                unit: Some("ssh.service".to_string()),
                priority: Some(6),
                message: Some("Started OpenSSH server".to_string()),
            }
        );
        assert_eq!(parsed[1].unit, None);
        assert_eq!(parsed[1].priority, Some(3));
    }
}
