//! Read-only Podman CLI integration with compact, schema-stable responses.

use async_trait::async_trait;
use serde_json::{json, Map, Value};
use std::{process::Stdio, time::Duration};
use tokio::process::Command;

use crate::errors::AppError;

const MAX_OUTPUT_BYTES: usize = 2 * 1024 * 1024;
const COMMAND_TIMEOUT: Duration = Duration::from_secs(5);

/// Abstracts read-only Podman inspection so domain handlers remain testable.
#[async_trait]
pub trait PodmanProvider: Send + Sync {
    /// Returns a compact container inspection object without raw OCI metadata.
    async fn container_status(&self, container: &str) -> Result<Value, AppError>;
    /// Returns a compact pod inspection object without labels or annotations.
    async fn pod_status(&self, pod: &str) -> Result<Value, AppError>;
}

/// Podman provider backed by fixed-argument local CLI invocations.
#[derive(Debug, Default)]
pub struct CliPodmanProvider;

/// Validates a Podman name or ID before it is supplied as one process argument.
fn validate_identifier(value: &str, kind: &'static str) -> Result<(), AppError> {
    if value.is_empty()
        || value.len() > 256
        || !value.bytes().all(|byte| {
            byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-' | b':' | b'@')
        })
    {
        return Err(AppError::bad_request(
            if kind == "container" {
                "invalid_container"
            } else {
                "invalid_pod"
            },
            if kind == "container" {
                "container identifier is invalid"
            } else {
                "pod identifier is invalid"
            },
        ));
    }
    Ok(())
}

/// Executes Podman directly, enforcing timeout and bounded captured output.
async fn run_podman(args: &[&str], not_found_code: &'static str) -> Result<Value, AppError> {
    let output = tokio::time::timeout(
        COMMAND_TIMEOUT,
        Command::new("podman")
            .args(args)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true)
            .output(),
    )
    .await
    .map_err(|_| AppError::bad_request("podman_timeout", "Podman inspection timed out"))?
    .map_err(|err| {
        if err.kind() == std::io::ErrorKind::NotFound {
            AppError::bad_request("podman_unavailable", "Podman is unavailable")
        } else {
            AppError::internal(format!("failed to execute podman: {err}"))
        }
    })?;

    if output.stdout.len() > MAX_OUTPUT_BYTES || output.stderr.len() > MAX_OUTPUT_BYTES {
        return Err(AppError::bad_request(
            "podman_output_too_large",
            "Podman returned an oversized response",
        ));
    }
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr).to_ascii_lowercase();
        if stderr.contains("no such") || stderr.contains("not found") {
            return Err(AppError::bad_request(
                not_found_code,
                "Podman target was not found",
            ));
        }
        if stderr.contains("cannot connect") || stderr.contains("permission denied") {
            return Err(AppError::bad_request(
                "podman_unavailable",
                "Podman is unavailable",
            ));
        }
        return Err(AppError::bad_request(
            "podman_provider_error",
            "Podman inspection failed",
        ));
    }

    serde_json::from_slice(&output.stdout).map_err(|_| {
        AppError::bad_request("podman_invalid_response", "Podman returned malformed JSON")
    })
}

fn first_object(value: &Value) -> Result<&Map<String, Value>, AppError> {
    value
        .as_array()
        .and_then(|items| items.first())
        .or(Some(value))
        .and_then(Value::as_object)
        .ok_or_else(|| {
            AppError::bad_request("podman_invalid_response", "Podman returned malformed JSON")
        })
}

fn at(value: &Value, pointer: &str) -> Value {
    value.pointer(pointer).cloned().unwrap_or(Value::Null)
}

/// Maps raw container inspect JSON into the deliberately compact public DTO.
fn compact_container(raw: &Value) -> Result<Value, AppError> {
    let object = first_object(raw)?;
    let value = Value::Object(object.clone());
    let state = at(&value, "/State");
    let config = at(&value, "/Config");
    let host_config = at(&value, "/HostConfig");
    let mounts = value.get("Mounts").and_then(Value::as_array).cloned().unwrap_or_default()
        .into_iter().map(|mount| json!({
            "type": at(&mount, "/Type"), "source": at(&mount, "/Source"),
            "destination": at(&mount, "/Destination"), "read_only": at(&mount, "/RW").as_bool().map(|rw| !rw)
        })).collect::<Vec<_>>();
    let health = state
        .get("Health")
        .map(|health| json!({"status": at(health, "/Status")}));
    Ok(json!({
        "id": at(&value, "/Id"), "name": at(&value, "/Name"),
        "state": at(&state, "/Status"), "running": at(&state, "/Running"),
        "exit_code": at(&state, "/ExitCode"), "error": at(&state, "/Error"),
        "started_at": at(&state, "/StartedAt"), "finished_at": at(&state, "/FinishedAt"),
        "created_at": value.get("Created").or_else(|| value.get("CreatedAt")).cloned(),
        "restart_count": value.get("RestartCount").cloned(),
        "image": {"name": config.get("Image").cloned(), "id": value.get("Image").or_else(|| value.get("ImageDigest")).cloned()},
        "configured_user": config.get("User").cloned(),
        "runtime_identity": {"uid": at(&value, "/State/UID"), "gid": at(&value, "/State/GID"), "host_uid": at(&value, "/State/HostUID"), "host_gid": at(&value, "/State/HostGID")},
        "read_only_rootfs": host_config.get("ReadonlyRootfs").cloned(), "mounts": mounts,
        "health": health, "health_config": config.get("Healthcheck").cloned(),
        "command": config.get("Cmd").cloned(), "create_command": value.get("CreateCommand").cloned(),
        "pod_id": value.get("Pod").or_else(|| value.get("PodId")).cloned()
    }))
}

/// Maps raw pod inspect JSON into the compact public DTO.
fn compact_pod(raw: &Value) -> Result<Value, AppError> {
    let object = first_object(raw)?;
    let value = Value::Object(object.clone());
    let containers = value.get("Containers").and_then(Value::as_array).cloned().unwrap_or_default()
        .into_iter().map(|item| json!({"id": at(&item, "/Id"), "name": at(&item, "/Name"), "state": at(&item, "/State")})).collect::<Vec<_>>();
    Ok(json!({
        "id": value.get("Id").or_else(|| value.get("ID")).cloned(), "name": value.get("Name").cloned(),
        "state": value.get("State").cloned(), "created_at": value.get("Created").or_else(|| value.get("CreatedAt")).cloned(),
        "restart_policy": value.pointer("/RestartPolicy").cloned(),
        "infra_container_id": value.get("InfraContainerID").or_else(|| value.get("InfraContainerId")).cloned(),
        "shared_namespaces": value.get("SharedNamespaces").cloned(), "containers": containers
    }))
}

#[async_trait]
impl PodmanProvider for CliPodmanProvider {
    async fn container_status(&self, container: &str) -> Result<Value, AppError> {
        validate_identifier(container, "container")?;
        compact_container(
            &run_podman(&["container", "inspect", container], "container_not_found").await?,
        )
    }

    async fn pod_status(&self, pod: &str) -> Result<Value, AppError> {
        validate_identifier(pod, "pod")?;
        compact_pod(&run_podman(&["pod", "inspect", pod], "pod_not_found").await?)
    }
}
