//! MCP handlers for compact read-only Podman inspection.

use serde::Deserialize;
use serde_json::{json, Value};

use crate::{
    domain::responses::tool_success_response,
    mcp::rpc::{app_error_to_json_rpc, json_rpc_invalid_params},
    AppState,
};

#[derive(Debug, Deserialize)]
struct ContainerParams {
    container: String,
}
#[derive(Debug, Deserialize)]
struct PodParams {
    pod: String,
}

/// Handles `get_container_status` after strict argument decoding.
pub async fn handle_container(
    state: &AppState,
    id: Option<Value>,
    arguments: Option<serde_json::Map<String, Value>>,
) -> Value {
    let params: ContainerParams = match serde_json::from_value(json!(arguments.unwrap_or_default()))
    {
        Ok(value) => value,
        Err(_) => return json_rpc_invalid_params(id),
    };
    match state
        .podman_provider
        .container_status(&params.container)
        .await
    {
        Ok(status) => tool_success_response(
            id,
            "Returned container status".to_string(),
            serde_json::Map::from_iter([("container".to_string(), status)]),
        ),
        Err(err) => app_error_to_json_rpc(id, err),
    }
}

/// Handles `get_pod_status` after strict argument decoding.
pub async fn handle_pod(
    state: &AppState,
    id: Option<Value>,
    arguments: Option<serde_json::Map<String, Value>>,
) -> Value {
    let params: PodParams = match serde_json::from_value(json!(arguments.unwrap_or_default())) {
        Ok(value) => value,
        Err(_) => return json_rpc_invalid_params(id),
    };
    match state.podman_provider.pod_status(&params.pod).await {
        Ok(status) => tool_success_response(
            id,
            "Returned pod status".to_string(),
            serde_json::Map::from_iter([("pod".to_string(), status)]),
        ),
        Err(err) => app_error_to_json_rpc(id, err),
    }
}
