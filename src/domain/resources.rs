//! Model Context Protocol static resource providers
//!
//! Exposes host system snapshots as file-like resources under `resource://` URIs.

use chrono::{Duration, Utc};
use rust_mcp_sdk::schema::{
    ReadResourceContent, ReadResourceRequestParams, ReadResourceResult, Resource,
    TextResourceContents,
};
use serde_json::{json, Value};

use crate::domain::utils::{filter_services_by_state, DEFAULT_LOG_LIMIT};
use crate::mcp::rpc::{
    app_error_to_json_rpc, json_rpc_error, json_rpc_error_with_data, json_rpc_result,
};
use crate::{systemd_client::LogQuery, AppState};

pub const SERVICES_RESOURCE_URI: &str = "resource://services/snapshot";
pub const FAILED_SERVICES_RESOURCE_URI: &str = "resource://services/failed";
pub const LOGS_RESOURCE_URI: &str = "resource://logs/recent";

pub fn build_resources_list() -> Vec<Resource> {
    vec![
        Resource {
            annotations: None,
            description: Some("Current systemd service statuses".to_string()),
            icons: vec![],
            meta: None,
            mime_type: Some("application/json".to_string()),
            name: "Service Snapshot".to_string(),
            size: None,
            title: None,
            uri: SERVICES_RESOURCE_URI.to_string(),
        },
        Resource {
            annotations: None,
            description: Some("Current failed systemd service statuses".to_string()),
            icons: vec![],
            meta: None,
            mime_type: Some("application/json".to_string()),
            name: "Failed Service Snapshot".to_string(),
            size: None,
            title: None,
            uri: FAILED_SERVICES_RESOURCE_URI.to_string(),
        },
        Resource {
            annotations: None,
            description: Some("Recent journald logs for the last hour".to_string()),
            icons: vec![],
            meta: None,
            mime_type: Some("application/json".to_string()),
            name: "Recent Logs Snapshot".to_string(),
            size: None,
            title: None,
            uri: LOGS_RESOURCE_URI.to_string(),
        },
    ]
}

pub async fn handle_resources_read(
    state: &AppState,
    id: Option<Value>,
    params: Option<Value>,
) -> Value {
    let Some(raw_params) = params else {
        return json_rpc_error(id, -32602, "Invalid params");
    };

    let resource_read: ReadResourceRequestParams = match serde_json::from_value(raw_params) {
        Ok(value) => value,
        Err(_) => return json_rpc_error(id, -32602, "Invalid params"),
    };

    match resource_read.uri.as_str() {
        SERVICES_RESOURCE_URI => match state.unit_provider.list_service_units().await {
            Ok(services) => {
                let structured_content = json!({ "services": services });
                let result = serde_json::to_value(ReadResourceResult {
                    contents: vec![ReadResourceContent::from(TextResourceContents {
                        meta: None,
                        mime_type: Some("application/json".to_string()),
                        text: structured_content.to_string(),
                        uri: SERVICES_RESOURCE_URI.to_string(),
                    })],
                    meta: None,
                })
                .expect("read services result serialization");

                json_rpc_result(id, result)
            }
            Err(err) => app_error_to_json_rpc(id, err),
        },
        FAILED_SERVICES_RESOURCE_URI => match state.unit_provider.list_service_units().await {
            Ok(services) => {
                let services = filter_services_by_state(services, Some("failed"));
                let structured_content = json!({ "services": services });
                let result = serde_json::to_value(ReadResourceResult {
                    contents: vec![ReadResourceContent::from(TextResourceContents {
                        meta: None,
                        mime_type: Some("application/json".to_string()),
                        text: structured_content.to_string(),
                        uri: FAILED_SERVICES_RESOURCE_URI.to_string(),
                    })],
                    meta: None,
                })
                .expect("read failed services result serialization");

                json_rpc_result(id, result)
            }
            Err(err) => app_error_to_json_rpc(id, err),
        },
        LOGS_RESOURCE_URI => {
            let end_utc = Utc::now();
            let start_utc = end_utc - Duration::hours(1);
            let query = LogQuery {
                priority: None,
                unit: None,
                exclude_units: vec![],
                grep: None,
                order: crate::systemd_client::LogOrder::Desc,
                start_utc: Some(start_utc),
                end_utc: Some(end_utc),
                limit: DEFAULT_LOG_LIMIT,
            };

            match state.unit_provider.list_journal_logs(&query).await {
                Ok(log_result) => {
                    let structured_content = json!({ "logs": log_result.entries });
                    let result = serde_json::to_value(ReadResourceResult {
                        contents: vec![ReadResourceContent::from(TextResourceContents {
                            meta: None,
                            mime_type: Some("application/json".to_string()),
                            text: structured_content.to_string(),
                            uri: LOGS_RESOURCE_URI.to_string(),
                        })],
                        meta: None,
                    })
                    .expect("read logs result serialization");

                    json_rpc_result(id, result)
                }
                Err(err) => app_error_to_json_rpc(id, err),
            }
        }
        _ => json_rpc_error_with_data(
            id,
            -32601,
            "Method not found",
            Some(json!({
                "code": "resource_not_found",
                "message": "unknown resource uri",
                "details": {
                    "uri": resource_read.uri,
                },
            })),
        ),
    }
}
