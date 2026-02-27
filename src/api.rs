use axum::{extract::State, Json};
use serde::Serialize;

use crate::{errors::AppError, systemd_client::UnitStatus, AppState};

#[derive(Debug, Serialize)]
pub struct HealthResponse {
    pub status: &'static str,
}

pub async fn health() -> Json<HealthResponse> {
    Json(HealthResponse { status: "ok" })
}

pub async fn list_units(State(state): State<AppState>) -> Result<Json<Vec<UnitStatus>>, AppError> {
    let units = state.unit_provider.list_service_units().await?;
    Ok(Json(units))
}
