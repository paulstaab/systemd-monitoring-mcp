use axum::{Json, extract::State};
use serde::Serialize;

use crate::{AppState, errors::AppError, systemd_client::UnitStatus};

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
