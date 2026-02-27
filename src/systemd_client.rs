use async_trait::async_trait;
use chrono::{DateTime, SecondsFormat, Utc};
use serde::Serialize;
use systemd::{daemon, journal};
use thiserror::Error;
use zbus::{zvariant::OwnedObjectPath, Connection, Proxy};

use crate::errors::AppError;

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct UnitStatus {
    pub name: String,
    pub state: String,
    pub description: Option<String>,
}

#[derive(Debug, Clone)]
pub struct LogQuery {
    pub priority: Option<String>,
    pub unit: Option<String>,
    pub start_utc: Option<DateTime<Utc>>,
    pub end_utc: Option<DateTime<Utc>>,
    pub limit: usize,
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

fn read_journal_logs(query: &LogQuery) -> Result<Vec<JournalLogEntry>, AppError> {
    let mut reader = journal::OpenOptions::default()
        .open()
        .map_err(|err| AppError::internal(format!("failed to open journald reader: {err}")))?;

    if let Some(unit) = &query.unit {
        reader
            .match_add("_SYSTEMD_UNIT", unit.as_bytes())
            .map_err(|err| AppError::internal(format!("failed to apply unit filter: {err}")))?;
    }

    if let Some(end_utc) = query.end_utc {
        let end_unix_usec = end_utc.timestamp_micros();
        if let Ok(end_unix_usec) = u64::try_from(end_unix_usec) {
            reader.seek_realtime_usec(end_unix_usec).map_err(|err| {
                AppError::internal(format!("failed to seek journald end timestamp: {err}"))
            })?;
        } else {
            reader.seek_head().map_err(|err| {
                AppError::internal(format!("failed to seek journald head: {err}"))
            })?;
        }
    } else {
        reader
            .seek_tail()
            .map_err(|err| AppError::internal(format!("failed to seek journald tail: {err}")))?;
    }

    let threshold = query
        .priority
        .as_deref()
        .and_then(|value| value.parse::<u8>().ok());
    let start_unix_usec = query.start_utc.map(|value| value.timestamp_micros());
    let end_unix_usec = query.end_utc.map(|value| value.timestamp_micros());

    let mut entries = Vec::new();

    while entries.len() < query.limit {
        let advanced = reader
            .previous()
            .map_err(|err| AppError::internal(format!("failed to read journald entry: {err}")))?;
        if advanced == 0 {
            break;
        }

        let timestamp_unix_usec_u64 = reader.timestamp_usec().map_err(|err| {
            AppError::internal(format!("failed to read journald timestamp: {err}"))
        })?;
        let Ok(timestamp_unix_usec) = i64::try_from(timestamp_unix_usec_u64) else {
            continue;
        };

        if let Some(start) = start_unix_usec {
            if timestamp_unix_usec < start {
                break;
            }
        }

        if let Some(end) = end_unix_usec {
            if timestamp_unix_usec > end {
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
        let unit = read_journal_field(&mut reader, "_SYSTEMD_UNIT")?;
        let message = read_journal_field(&mut reader, "MESSAGE")?;

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
    fn journal_log_entry_keeps_expected_shape() {
        let sample = JournalLogEntry {
            timestamp_utc: "2025-01-01T00:00:00.000Z".to_string(),
            timestamp_unix_usec: 1_735_689_600_000_000,
            unit: Some("ssh.service".to_string()),
            priority: Some(6),
            message: Some("Started OpenSSH server".to_string()),
        };

        assert_eq!(sample.unit.as_deref(), Some("ssh.service"));
        assert_eq!(sample.priority, Some(6));
    }
}
