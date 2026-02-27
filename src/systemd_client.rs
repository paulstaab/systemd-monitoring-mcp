use async_trait::async_trait;
use serde::Serialize;
use zbus::{zvariant::OwnedObjectPath, Connection, Proxy};

use crate::errors::AppError;

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct UnitStatus {
    pub name: String,
    pub state: String,
    pub description: Option<String>,
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

#[async_trait]
pub trait UnitProvider: Send + Sync {
    async fn list_service_units(&self) -> Result<Vec<UnitStatus>, AppError>;
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

#[cfg(test)]
mod tests {
    use super::{map_and_sort_service_units, RawUnit};

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
}
