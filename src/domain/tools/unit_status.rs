//! Detailed systemd service inspection MCP handler.

use serde::Deserialize;
use serde_json::{json, Value};

use crate::{
    domain::{
        responses::tool_success_response,
        utils::{normalize_scope, normalize_unit},
    },
    errors::AppError,
    mcp::rpc::{app_error_to_json_rpc, json_rpc_invalid_params},
    systemd_client::UnitScope,
    AppState,
};

#[derive(Debug, Deserialize)]
struct Params {
    unit: String,
    scope: Option<String>,
    transition_limit: Option<u32>,
}

/// Validates and handles `get_unit_status` for a concrete service manager scope.
pub async fn handle(
    state: &AppState,
    id: Option<Value>,
    arguments: Option<serde_json::Map<String, Value>>,
) -> Value {
    let params: Params = match serde_json::from_value(json!(arguments.unwrap_or_default())) {
        Ok(value) => value,
        Err(_) => return json_rpc_invalid_params(id),
    };
    let unit = match normalize_unit(Some(params.unit)) {
        Ok(Some(value)) if value.ends_with(".service") => value,
        _ => {
            return app_error_to_json_rpc(
                id,
                AppError::bad_request("invalid_unit", "unit must be a valid .service name"),
            )
        }
    };
    let scope = match normalize_scope(params.scope) {
        Ok(UnitScope::System) => UnitScope::System,
        Ok(UnitScope::User) => UnitScope::User,
        _ => {
            return app_error_to_json_rpc(
                id,
                AppError::bad_request("invalid_scope", "scope must be system or user"),
            )
        }
    };
    let limit = params.transition_limit.unwrap_or(20);
    if !(1..=100).contains(&limit) {
        return app_error_to_json_rpc(
            id,
            AppError::bad_request(
                "invalid_transition_limit",
                "transition_limit must be between 1 and 100",
            ),
        );
    }
    match state
        .unit_provider
        .get_unit_status(&unit, scope, limit as usize)
        .await
    {
        Ok(status) => tool_success_response(
            id,
            "Returned unit status".to_string(),
            serde_json::Map::from_iter([("status".to_string(), status)]),
        ),
        Err(err) => app_error_to_json_rpc(id, err),
    }
}
