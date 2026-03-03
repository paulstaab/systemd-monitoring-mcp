use std::sync::Arc;

use axum::{
    body::Body,
    http::{header, Request, StatusCode},
};
use http_body_util::BodyExt;
use tower::ServiceExt;

use crate::systemd_client::{
    JournalLogEntry, LogOrder, LogQuery, LogQueryResult, TimerStatus, UnitProvider, UnitStatus,
};

use super::*;

struct MockProvider;

#[async_trait::async_trait]
impl UnitProvider for MockProvider {
    async fn list_service_units(&self) -> Result<Vec<UnitStatus>, crate::errors::AppError> {
        Ok(vec![
            UnitStatus {
                unit: "z.service".to_string(),
                description: "".to_string(),
                load_state: "loaded".to_string(),
                active_state: "active".to_string(),
                sub_state: "running".to_string(),
                unit_file_state: Some("enabled".to_string()),
                since_utc: Some("2026-02-27T00:00:00.000Z".to_string()),
                main_pid: Some(3001),
                exec_main_status: Some(0),
                result: Some("success".to_string()),
            },
            UnitStatus {
                unit: "a.service".to_string(),
                description: "A service".to_string(),
                load_state: "loaded".to_string(),
                active_state: "inactive".to_string(),
                sub_state: "dead".to_string(),
                unit_file_state: Some("disabled".to_string()),
                since_utc: None,
                main_pid: None,
                exec_main_status: None,
                result: None,
            },
            UnitStatus {
                unit: "b.service".to_string(),
                description: "B service".to_string(),
                load_state: "loaded".to_string(),
                active_state: "failed".to_string(),
                sub_state: "failed".to_string(),
                unit_file_state: Some("enabled".to_string()),
                since_utc: Some("2026-02-28T00:00:00.000Z".to_string()),
                main_pid: Some(4001),
                exec_main_status: Some(1),
                result: Some("exit-code".to_string()),
            },
        ])
    }

    async fn list_journal_logs(
        &self,
        query: &LogQuery,
    ) -> Result<LogQueryResult, crate::errors::AppError> {
        let mut entries = vec![
            JournalLogEntry {
                timestamp_utc: "2026-02-27T00:00:00.000Z".to_string(),
                unit: Some("ssh.service".to_string()),
                priority: Some("6".to_string()),
                hostname: Some("test-host".to_string()),
                pid: Some(2222),
                message: Some("Started OpenSSH server".to_string()),
                cursor: Some("s=cursor;i=12".to_string()),
            },
            JournalLogEntry {
                timestamp_utc: "2026-02-27T00:30:00.000Z".to_string(),
                unit: Some("cron.service".to_string()),
                priority: Some("5".to_string()),
                hostname: Some("test-host".to_string()),
                pid: Some(3333),
                message: Some("Cron wake-up".to_string()),
                cursor: Some("s=cursor;i=13".to_string()),
            },
            JournalLogEntry {
                timestamp_utc: "2026-02-27T00:45:00.000Z".to_string(),
                unit: Some("app.service".to_string()),
                priority: Some("4".to_string()),
                hostname: Some("test-host".to_string()),
                pid: Some(4444),
                message: Some("Application warning".to_string()),
                cursor: Some("s=cursor;i=14".to_string()),
            },
        ];

        let scanned = entries.len();

        if let Some(unit_filter) = query.unit.as_deref() {
            entries.retain(|entry| entry.unit.as_deref() == Some(unit_filter));
        }

        if !query.exclude_units.is_empty() {
            entries.retain(|entry| {
                let Some(unit) = entry.unit.as_deref() else {
                    return true;
                };
                !query
                    .exclude_units
                    .iter()
                    .any(|excluded| excluded.eq_ignore_ascii_case(unit))
            });
        }

        entries.sort_by(|left, right| left.timestamp_utc.cmp(&right.timestamp_utc));
        if query.order == LogOrder::Desc {
            entries.reverse();
        }

        entries.truncate(query.limit);

        Ok(LogQueryResult {
            entries,
            total_scanned: Some(scanned),
        })
    }

    async fn list_timer_units(&self) -> Result<Vec<TimerStatus>, crate::errors::AppError> {
        Ok(vec![
            TimerStatus {
                unit: "backup.timer".to_string(),
                load_state: "loaded".to_string(),
                active_state: "active".to_string(),
                sub_state: "waiting".to_string(),
                unit_file_state: Some("enabled".to_string()),
                next_run_utc: Some("2099-01-01T00:00:00.000Z".to_string()),
                last_run_utc: Some("2020-01-01T00:00:00.000Z".to_string()),
                trigger_unit: Some("backup.service".to_string()),
                persistent: Some(true),
                result: Some("success".to_string()),
            },
            TimerStatus {
                unit: "stale.timer".to_string(),
                load_state: "loaded".to_string(),
                active_state: "inactive".to_string(),
                sub_state: "dead".to_string(),
                unit_file_state: Some("disabled".to_string()),
                next_run_utc: None,
                last_run_utc: Some("2019-01-01T00:00:00.000Z".to_string()),
                trigger_unit: Some("stale.service".to_string()),
                persistent: None,
                result: None,
            },
            TimerStatus {
                unit: "overdue.timer".to_string(),
                load_state: "loaded".to_string(),
                active_state: "active".to_string(),
                sub_state: "waiting".to_string(),
                unit_file_state: Some("enabled".to_string()),
                next_run_utc: Some("2020-01-01T00:00:00.000Z".to_string()),
                last_run_utc: Some("2019-12-31T23:00:00.000Z".to_string()),
                trigger_unit: Some("overdue.service".to_string()),
                persistent: Some(true),
                result: Some("success".to_string()),
            },
        ])
    }
}

fn app() -> Router {
    let state = AppState::new("token-1234567890ab".to_string(), Arc::new(MockProvider));
    build_app(state)
}

#[tokio::test]
async fn health_is_public() {
    let response = app()
        .oneshot(
            Request::builder()
                .uri("/health")
                .method("GET")
                .body(Body::empty())
                .expect("request build"),
        )
        .await
        .expect("request execution");

    assert_eq!(response.status(), StatusCode::OK);
    let body = response
        .into_body()
        .collect()
        .await
        .expect("collect body")
        .to_bytes();
    assert_eq!(body, "{\"status\":\"ok\"}");
}

#[tokio::test]
async fn services_route_is_not_found() {
    let response = app()
        .oneshot(
            Request::builder()
                .uri("/services")
                .method("GET")
                .body(Body::empty())
                .expect("request build"),
        )
        .await
        .expect("request execution");

    assert_eq!(response.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn logs_route_is_not_found() {
    let response = app()
        .oneshot(
            Request::builder()
                .uri("/logs")
                .method("GET")
                .body(Body::empty())
                .expect("request build"),
        )
        .await
        .expect("request execution");

    assert_eq!(response.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn discovery_is_public() {
    let response = app()
        .oneshot(
            Request::builder()
                .uri("/.well-known/mcp")
                .method("GET")
                .body(Body::empty())
                .expect("request build"),
        )
        .await
        .expect("request execution");

    assert_eq!(response.status(), StatusCode::OK);
    let body = response
        .into_body()
        .collect()
        .await
        .expect("collect body")
        .to_bytes();
    let body_json: serde_json::Value = serde_json::from_slice(&body).expect("valid json response");
    assert_eq!(body_json["mcp_endpoint"], "/mcp");
}

#[tokio::test]
async fn root_get_is_not_found() {
    let response = app()
        .oneshot(
            Request::builder()
                .uri("/")
                .method("GET")
                .body(Body::empty())
                .expect("request build"),
        )
        .await
        .expect("request execution");

    assert_eq!(response.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn mcp_requires_token() {
    let response = app()
        .oneshot(
            Request::builder()
                .uri("/mcp")
                .method("POST")
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(r#"{"jsonrpc":"2.0","id":1,"method":"unknown"}"#))
                .expect("request build"),
        )
        .await
        .expect("request execution");

    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn mcp_non_bearer_auth_is_invalid_token() {
    let response = app()
        .oneshot(
            Request::builder()
                .uri("/mcp")
                .method("POST")
                .header(header::CONTENT_TYPE, "application/json")
                .header(header::AUTHORIZATION, "Basic dXNlcjpwYXNz")
                .body(Body::from(r#"{"jsonrpc":"2.0","id":1,"method":"unknown"}"#))
                .expect("request build"),
        )
        .await
        .expect("request execution");

    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    let body = response
        .into_body()
        .collect()
        .await
        .expect("collect body")
        .to_bytes();
    let body_json: serde_json::Value = serde_json::from_slice(&body).expect("valid json response");

    assert_eq!(body_json["code"], "invalid_token");
}

#[tokio::test]
async fn mcp_unknown_method_returns_method_not_found() {
    let response = app()
        .oneshot(
            Request::builder()
                .uri("/mcp")
                .method("POST")
                .header(header::CONTENT_TYPE, "application/json")
                .header(header::AUTHORIZATION, "Bearer token-1234567890ab")
                .body(Body::from(r#"{"jsonrpc":"2.0","id":1,"method":"unknown"}"#))
                .expect("request build"),
        )
        .await
        .expect("request execution");

    assert_eq!(response.status(), StatusCode::OK);
    let body = response
        .into_body()
        .collect()
        .await
        .expect("collect body")
        .to_bytes();
    assert_eq!(
            body,
            "{\"error\":{\"code\":-32601,\"message\":\"Method not found\"},\"id\":1,\"jsonrpc\":\"2.0\"}"
        );
}

#[tokio::test]
async fn mcp_initialize_returns_result() {
    let response = app()
            .oneshot(
                Request::builder()
                    .uri("/mcp")
                    .method("POST")
                    .header(header::CONTENT_TYPE, "application/json")
                    .header(header::AUTHORIZATION, "Bearer token-1234567890ab")
                    .body(Body::from(
                        r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2024-11-05","clientInfo":{"name":"test-client","version":"1.0.0"},"capabilities":{}}}"#,
                    ))
                    .expect("request build"),
            )
            .await
            .expect("request execution");

    assert_eq!(response.status(), StatusCode::OK);
    let body = response
        .into_body()
        .collect()
        .await
        .expect("collect body")
        .to_bytes();
    let body_json: serde_json::Value = serde_json::from_slice(&body).expect("valid json response");

    assert_eq!(body_json["jsonrpc"], "2.0");
    assert_eq!(body_json["id"], 1);
    assert_eq!(body_json["result"]["protocolVersion"], "2024-11-05");
    assert_eq!(
        body_json["result"]["serverInfo"]["name"],
        env!("CARGO_PKG_NAME")
    );
    assert_eq!(
        body_json["result"]["serverInfo"]["version"],
        env!("CARGO_PKG_VERSION")
    );
    assert!(body_json["result"]["capabilities"]["tools"].is_object());
    assert!(body_json["result"]["capabilities"]["resources"].is_object());
    assert!(body_json["result"]["capabilities"]["prompts"].is_null());
}

#[tokio::test]
async fn mcp_initialize_accepts_modern_protocol_version() {
    let response = app()
            .oneshot(
                Request::builder()
                    .uri("/mcp")
                    .method("POST")
                    .header(header::CONTENT_TYPE, "application/json")
                    .header(header::AUTHORIZATION, "Bearer token-1234567890ab")
                    .body(Body::from(
                        r#"{"jsonrpc":"2.0","id":11,"method":"initialize","params":{"protocolVersion":"2025-03-26","clientInfo":{"name":"test-client","version":"1.0.0"},"capabilities":{}}}"#,
                    ))
                    .expect("request build"),
            )
            .await
            .expect("request execution");

    assert_eq!(response.status(), StatusCode::OK);
    let body = response
        .into_body()
        .collect()
        .await
        .expect("collect body")
        .to_bytes();
    let body_json: serde_json::Value = serde_json::from_slice(&body).expect("valid json response");

    assert_eq!(body_json["jsonrpc"], "2.0");
    assert_eq!(body_json["id"], 11);
    assert_eq!(body_json["result"]["protocolVersion"], "2025-03-26");
}

#[tokio::test]
async fn root_post_does_not_provide_mcp() {
    let response = app()
        .oneshot(
            Request::builder()
                .uri("/")
                .method("POST")
                .header(header::CONTENT_TYPE, "application/json")
                .header(header::AUTHORIZATION, "Bearer token-1234567890ab")
                .body(Body::from(
                    r#"{"jsonrpc":"2.0","id":1,"method":"initialize"}"#,
                ))
                .expect("request build"),
        )
        .await
        .expect("request execution");

    assert_eq!(response.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn mcp_get_is_method_not_allowed() {
    let response = app()
        .oneshot(
            Request::builder()
                .uri("/mcp")
                .method("GET")
                .header(header::AUTHORIZATION, "Bearer token-1234567890ab")
                .body(Body::empty())
                .expect("request build"),
        )
        .await
        .expect("request execution");

    assert_eq!(response.status(), StatusCode::METHOD_NOT_ALLOWED);
}

#[tokio::test]
async fn mcp_tools_list_returns_required_tools() {
    let response = app()
        .oneshot(
            Request::builder()
                .uri("/mcp")
                .method("POST")
                .header(header::CONTENT_TYPE, "application/json")
                .header(header::AUTHORIZATION, "Bearer token-1234567890ab")
                .body(Body::from(
                    r#"{"jsonrpc":"2.0","id":2,"method":"tools/list","params":{}}"#,
                ))
                .expect("request build"),
        )
        .await
        .expect("request execution");

    assert_eq!(response.status(), StatusCode::OK);
    let body = response
        .into_body()
        .collect()
        .await
        .expect("collect body")
        .to_bytes();
    let body_json: serde_json::Value = serde_json::from_slice(&body).expect("valid json response");

    assert_eq!(body_json["jsonrpc"], "2.0");
    assert_eq!(body_json["id"], 2);
    assert!(body_json["result"]["tools"].is_array());
    assert_eq!(body_json["result"]["tools"][0]["name"], "list_services");
    assert_eq!(body_json["result"]["tools"][1]["name"], "list_timers");
    assert_eq!(body_json["result"]["tools"][2]["name"], "list_logs");
}

#[tokio::test]
async fn mcp_tools_call_list_timers_returns_structured_content() {
    let response = app()
            .oneshot(
                Request::builder()
                    .uri("/mcp")
                    .method("POST")
                    .header(header::CONTENT_TYPE, "application/json")
                    .header(header::AUTHORIZATION, "Bearer token-1234567890ab")
                    .body(Body::from(
                        r#"{"jsonrpc":"2.0","id":318,"method":"tools/call","params":{"name":"list_timers","arguments":{"include_persistent":true}}}"#,
                    ))
                    .expect("request build"),
            )
            .await
            .expect("request execution");

    assert_eq!(response.status(), StatusCode::OK);
    let body = response
        .into_body()
        .collect()
        .await
        .expect("collect body")
        .to_bytes();
    let body_json: serde_json::Value = serde_json::from_slice(&body).expect("valid json response");

    assert_eq!(body_json["jsonrpc"], "2.0");
    assert_eq!(body_json["id"], 318);
    assert!(body_json["result"]["structuredContent"]["timers"].is_array());
    assert!(body_json["result"]["structuredContent"]["total_scanned"].is_number());
    assert!(body_json["result"]["structuredContent"]["returned"].is_number());
    assert!(body_json["result"]["structuredContent"]["truncated"].is_boolean());
    assert!(body_json["result"]["structuredContent"]["generated_at_utc"].is_string());
    assert!(body_json["result"]["structuredContent"]["timers"][0]["unit"].is_string());
    assert!(body_json["result"]["structuredContent"]["timers"][0]["active_state"].is_string());
    assert!(body_json["result"]["structuredContent"]["timers"][0]["overdue"].is_boolean());
    assert!(
        body_json["result"]["structuredContent"]["timers"][0]["time_until_next_sec"].is_number()
            || body_json["result"]["structuredContent"]["timers"][0]["time_until_next_sec"]
                .is_null()
    );
}

#[tokio::test]
async fn mcp_tools_call_list_timers_overdue_only_filters_rows() {
    let response = app()
            .oneshot(
                Request::builder()
                    .uri("/mcp")
                    .method("POST")
                    .header(header::CONTENT_TYPE, "application/json")
                    .header(header::AUTHORIZATION, "Bearer token-1234567890ab")
                    .body(Body::from(
                        r#"{"jsonrpc":"2.0","id":319,"method":"tools/call","params":{"name":"list_timers","arguments":{"overdue_only":true}}}"#,
                    ))
                    .expect("request build"),
            )
            .await
            .expect("request execution");

    assert_eq!(response.status(), StatusCode::OK);
    let body = response
        .into_body()
        .collect()
        .await
        .expect("collect body")
        .to_bytes();
    let body_json: serde_json::Value = serde_json::from_slice(&body).expect("valid json response");

    assert_eq!(body_json["jsonrpc"], "2.0");
    assert_eq!(body_json["id"], 319);
    assert_eq!(body_json["result"]["structuredContent"]["returned"], 1);
    assert_eq!(
        body_json["result"]["structuredContent"]["timers"][0]["unit"],
        "overdue.timer"
    );
    assert_eq!(
        body_json["result"]["structuredContent"]["timers"][0]["overdue"],
        true
    );
}

#[tokio::test]
async fn mcp_tools_call_list_timers_summary_with_overdue_only_returns_expected_counts() {
    let response = app()
            .oneshot(
                Request::builder()
                    .uri("/mcp")
                    .method("POST")
                    .header(header::CONTENT_TYPE, "application/json")
                    .header(header::AUTHORIZATION, "Bearer token-1234567890ab")
                    .body(Body::from(
                        r#"{"jsonrpc":"2.0","id":320,"method":"tools/call","params":{"name":"list_timers","arguments":{"summary":true,"overdue_only":true}}}"#,
                    ))
                    .expect("request build"),
            )
            .await
            .expect("request execution");

    assert_eq!(response.status(), StatusCode::OK);
    let body = response
        .into_body()
        .collect()
        .await
        .expect("collect body")
        .to_bytes();
    let body_json: serde_json::Value = serde_json::from_slice(&body).expect("valid json response");

    assert_eq!(body_json["jsonrpc"], "2.0");
    assert_eq!(body_json["id"], 320);
    assert_eq!(
        body_json["result"]["structuredContent"]["summary"]["overdue_count"],
        1
    );
    assert_eq!(body_json["result"]["structuredContent"]["returned"], 1);
    assert!(
        body_json["result"]["structuredContent"]["summary"]["failed_or_problem_timers"]
            .as_array()
            .map(|rows| rows.iter().any(|row| row["unit"] == "overdue.timer"))
            .unwrap_or(false)
    );
}

#[tokio::test]
async fn mcp_tools_call_list_services_returns_structured_content() {
    let response = app()
            .oneshot(
                Request::builder()
                    .uri("/mcp")
                    .method("POST")
                    .header(header::CONTENT_TYPE, "application/json")
                    .header(header::AUTHORIZATION, "Bearer token-1234567890ab")
                    .body(Body::from(
                        r#"{"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"list_services","arguments":{}}}"#,
                    ))
                    .expect("request build"),
            )
            .await
            .expect("request execution");

    assert_eq!(response.status(), StatusCode::OK);
    let body = response
        .into_body()
        .collect()
        .await
        .expect("collect body")
        .to_bytes();
    let body_json: serde_json::Value = serde_json::from_slice(&body).expect("valid json response");

    assert_eq!(body_json["jsonrpc"], "2.0");
    assert_eq!(body_json["id"], 3);
    assert!(body_json["result"]["structuredContent"]["services"].is_array());
    assert!(body_json["result"]["structuredContent"]["total"].is_number());
    assert!(body_json["result"]["structuredContent"]["returned"].is_number());
    assert!(body_json["result"]["structuredContent"]["truncated"].is_boolean());
    assert!(body_json["result"]["structuredContent"]["generated_at_utc"].is_string());
    assert!(body_json["result"]["structuredContent"]["services"][0]["unit"].is_string());
    assert!(body_json["result"]["structuredContent"]["services"][0]["active_state"].is_string());
    assert!(body_json["result"]["content"].is_array());
}

#[tokio::test]
async fn mcp_tools_call_list_services_filters_by_state() {
    let response = app()
            .oneshot(
                Request::builder()
                    .uri("/mcp")
                    .method("POST")
                    .header(header::CONTENT_TYPE, "application/json")
                    .header(header::AUTHORIZATION, "Bearer token-1234567890ab")
                    .body(Body::from(
                        r#"{"jsonrpc":"2.0","id":32,"method":"tools/call","params":{"name":"list_services","arguments":{"state":"inactive"}}}"#,
                    ))
                    .expect("request build"),
            )
            .await
            .expect("request execution");

    assert_eq!(response.status(), StatusCode::OK);
    let body = response
        .into_body()
        .collect()
        .await
        .expect("collect body")
        .to_bytes();
    let body_json: serde_json::Value = serde_json::from_slice(&body).expect("valid json response");

    assert_eq!(body_json["jsonrpc"], "2.0");
    assert_eq!(body_json["id"], 32);
    assert_eq!(
        body_json["result"]["structuredContent"]["services"]
            .as_array()
            .map(Vec::len),
        Some(1)
    );
    assert_eq!(
        body_json["result"]["structuredContent"]["services"][0]["unit"],
        "a.service"
    );
}

#[tokio::test]
async fn mcp_tools_call_list_services_filters_by_name_contains() {
    let response = app()
            .oneshot(
                Request::builder()
                    .uri("/mcp")
                    .method("POST")
                    .header(header::CONTENT_TYPE, "application/json")
                    .header(header::AUTHORIZATION, "Bearer token-1234567890ab")
                    .body(Body::from(
                        r#"{"jsonrpc":"2.0","id":34,"method":"tools/call","params":{"name":"list_services","arguments":{"name_contains":"b."}}}"#,
                    ))
                    .expect("request build"),
            )
            .await
            .expect("request execution");

    assert_eq!(response.status(), StatusCode::OK);
    let body = response
        .into_body()
        .collect()
        .await
        .expect("collect body")
        .to_bytes();
    let body_json: serde_json::Value = serde_json::from_slice(&body).expect("valid json response");

    assert_eq!(body_json["jsonrpc"], "2.0");
    assert_eq!(body_json["id"], 34);
    assert_eq!(
        body_json["result"]["structuredContent"]["services"]
            .as_array()
            .map(Vec::len),
        Some(1)
    );
    assert_eq!(
        body_json["result"]["structuredContent"]["services"][0]["unit"],
        "b.service"
    );
}

#[tokio::test]
async fn mcp_tools_call_list_services_filters_by_state_and_name_contains() {
    let response = app()
            .oneshot(
                Request::builder()
                    .uri("/mcp")
                    .method("POST")
                    .header(header::CONTENT_TYPE, "application/json")
                    .header(header::AUTHORIZATION, "Bearer token-1234567890ab")
                    .body(Body::from(
                        r#"{"jsonrpc":"2.0","id":35,"method":"tools/call","params":{"name":"list_services","arguments":{"state":"failed","name_contains":"b"}}}"#,
                    ))
                    .expect("request build"),
            )
            .await
            .expect("request execution");

    assert_eq!(response.status(), StatusCode::OK);
    let body = response
        .into_body()
        .collect()
        .await
        .expect("collect body")
        .to_bytes();
    let body_json: serde_json::Value = serde_json::from_slice(&body).expect("valid json response");

    assert_eq!(body_json["jsonrpc"], "2.0");
    assert_eq!(body_json["id"], 35);
    assert_eq!(
        body_json["result"]["structuredContent"]["services"]
            .as_array()
            .map(Vec::len),
        Some(1)
    );
    assert_eq!(
        body_json["result"]["structuredContent"]["services"][0]["unit"],
        "b.service"
    );
    assert_eq!(
        body_json["result"]["structuredContent"]["services"][0]["active_state"],
        "failed"
    );
}

#[tokio::test]
async fn mcp_tools_call_list_services_rejects_invalid_state() {
    let response = app()
            .oneshot(
                Request::builder()
                    .uri("/mcp")
                    .method("POST")
                    .header(header::CONTENT_TYPE, "application/json")
                    .header(header::AUTHORIZATION, "Bearer token-1234567890ab")
                    .body(Body::from(
                        r#"{"jsonrpc":"2.0","id":33,"method":"tools/call","params":{"name":"list_services","arguments":{"state":"running"}}}"#,
                    ))
                    .expect("request build"),
            )
            .await
            .expect("request execution");

    assert_eq!(response.status(), StatusCode::OK);
    let body = response
        .into_body()
        .collect()
        .await
        .expect("collect body")
        .to_bytes();
    let body_json: serde_json::Value = serde_json::from_slice(&body).expect("valid json response");

    assert_eq!(body_json["jsonrpc"], "2.0");
    assert_eq!(body_json["id"], 33);
    assert_eq!(body_json["error"]["code"], -32602);
    assert_eq!(body_json["error"]["data"]["code"], "invalid_state");
}

#[tokio::test]
async fn mcp_tools_call_list_services_rejects_invalid_limit() {
    let response = app()
            .oneshot(
                Request::builder()
                    .uri("/mcp")
                    .method("POST")
                    .header(header::CONTENT_TYPE, "application/json")
                    .header(header::AUTHORIZATION, "Bearer token-1234567890ab")
                    .body(Body::from(
                        r#"{"jsonrpc":"2.0","id":36,"method":"tools/call","params":{"name":"list_services","arguments":{"limit":1001}}}"#,
                    ))
                    .expect("request build"),
            )
            .await
            .expect("request execution");

    assert_eq!(response.status(), StatusCode::OK);
    let body = response
        .into_body()
        .collect()
        .await
        .expect("collect body")
        .to_bytes();
    let body_json: serde_json::Value = serde_json::from_slice(&body).expect("valid json response");

    assert_eq!(body_json["jsonrpc"], "2.0");
    assert_eq!(body_json["id"], 36);
    assert_eq!(body_json["error"]["code"], -32602);
    assert_eq!(body_json["error"]["data"]["code"], "invalid_limit");
}

#[tokio::test]
async fn mcp_tools_call_list_logs_returns_structured_content() {
    let response = app()
            .oneshot(
                Request::builder()
                    .uri("/mcp")
                    .method("POST")
                    .header(header::CONTENT_TYPE, "application/json")
                    .header(header::AUTHORIZATION, "Bearer token-1234567890ab")
                    .body(Body::from(
                        r#"{"jsonrpc":"2.0","id":31,"method":"tools/call","params":{"name":"list_logs","arguments":{"start_utc":"2026-02-27T00:00:00Z","end_utc":"2026-02-27T01:00:00Z","limit":10}}}"#,
                    ))
                    .expect("request build"),
            )
            .await
            .expect("request execution");

    assert_eq!(response.status(), StatusCode::OK);
    let body = response
        .into_body()
        .collect()
        .await
        .expect("collect body")
        .to_bytes();
    let body_json: serde_json::Value = serde_json::from_slice(&body).expect("valid json response");

    assert_eq!(body_json["jsonrpc"], "2.0");
    assert_eq!(body_json["id"], 31);
    assert!(body_json["result"]["structuredContent"]["logs"].is_array());
    assert!(body_json["result"]["structuredContent"]["total_scanned"].is_number());
    assert!(body_json["result"]["structuredContent"]["returned"].is_number());
    assert!(body_json["result"]["structuredContent"]["truncated"].is_boolean());
    assert!(body_json["result"]["structuredContent"]["generated_at_utc"].is_string());
    assert!(body_json["result"]["structuredContent"]["window"]["start_utc"].is_string());
    assert!(body_json["result"]["structuredContent"]["window"]["end_utc"].is_string());
    assert!(body_json["result"]["structuredContent"]["logs"][0]["timestamp_utc"].is_string());
    assert!(body_json["result"]["structuredContent"]["logs"][0]["priority"].is_string());
    assert!(body_json["result"]["structuredContent"]["logs"][0]["hostname"].is_string());
    assert!(body_json["result"]["structuredContent"]["logs"][0]["pid"].is_number());
    assert!(body_json["result"]["structuredContent"]["logs"][0]["cursor"].is_string());
}

#[tokio::test]
async fn mcp_tools_call_list_services_summary_returns_compact_block() {
    let response = app()
            .oneshot(
                Request::builder()
                    .uri("/mcp")
                    .method("POST")
                    .header(header::CONTENT_TYPE, "application/json")
                    .header(header::AUTHORIZATION, "Bearer token-1234567890ab")
                    .body(Body::from(
                        r#"{"jsonrpc":"2.0","id":313,"method":"tools/call","params":{"name":"list_services","arguments":{"summary":true}}}"#,
                    ))
                    .expect("request build"),
            )
            .await
            .expect("request execution");

    assert_eq!(response.status(), StatusCode::OK);
    let body = response
        .into_body()
        .collect()
        .await
        .expect("collect body")
        .to_bytes();
    let body_json: serde_json::Value = serde_json::from_slice(&body).expect("valid json response");

    assert_eq!(body_json["jsonrpc"], "2.0");
    assert_eq!(body_json["id"], 313);
    assert!(
        body_json["result"]["structuredContent"]["summary"]["counts_by_active_state"].is_object()
    );
    assert!(body_json["result"]["structuredContent"]["summary"]["failed_units"].is_array());
    assert!(body_json["result"]["structuredContent"]["summary"]["degraded_hint"].is_string());
    assert!(body_json["result"]["structuredContent"]
        .get("services")
        .is_none());
}

#[tokio::test]
async fn mcp_tools_call_list_logs_summary_returns_compact_block() {
    let response = app()
            .oneshot(
                Request::builder()
                    .uri("/mcp")
                    .method("POST")
                    .header(header::CONTENT_TYPE, "application/json")
                    .header(header::AUTHORIZATION, "Bearer token-1234567890ab")
                    .body(Body::from(
                        r#"{"jsonrpc":"2.0","id":314,"method":"tools/call","params":{"name":"list_logs","arguments":{"start_utc":"2026-02-27T00:00:00Z","end_utc":"2026-02-27T01:00:00Z","summary":true,"limit":10}}}"#,
                    ))
                    .expect("request build"),
            )
            .await
            .expect("request execution");

    assert_eq!(response.status(), StatusCode::OK);
    let body = response
        .into_body()
        .collect()
        .await
        .expect("collect body")
        .to_bytes();
    let body_json: serde_json::Value = serde_json::from_slice(&body).expect("valid json response");

    assert_eq!(body_json["jsonrpc"], "2.0");
    assert_eq!(body_json["id"], 314);
    assert!(body_json["result"]["structuredContent"]["summary"]["counts_by_unit"].is_object());
    assert!(body_json["result"]["structuredContent"]["summary"]["counts_by_priority"].is_object());
    assert!(body_json["result"]["structuredContent"]["summary"]["top_messages"].is_array());
    assert!(body_json["result"]["structuredContent"]["summary"]["error_hotspots"].is_array());
    assert!(body_json["result"]["structuredContent"]
        .get("logs")
        .is_none());
}

#[tokio::test]
async fn mcp_tools_call_list_logs_honors_order_asc() {
    let response = app()
            .oneshot(
                Request::builder()
                    .uri("/mcp")
                    .method("POST")
                    .header(header::CONTENT_TYPE, "application/json")
                    .header(header::AUTHORIZATION, "Bearer token-1234567890ab")
                    .body(Body::from(
                        r#"{"jsonrpc":"2.0","id":311,"method":"tools/call","params":{"name":"list_logs","arguments":{"start_utc":"2026-02-27T00:00:00Z","end_utc":"2026-02-27T01:00:00Z","order":"asc","limit":10}}}"#,
                    ))
                    .expect("request build"),
            )
            .await
            .expect("request execution");

    assert_eq!(response.status(), StatusCode::OK);
    let body = response
        .into_body()
        .collect()
        .await
        .expect("collect body")
        .to_bytes();
    let body_json: serde_json::Value = serde_json::from_slice(&body).expect("valid json response");

    assert_eq!(body_json["jsonrpc"], "2.0");
    assert_eq!(body_json["id"], 311);
    let logs = body_json["result"]["structuredContent"]["logs"]
        .as_array()
        .expect("logs array");
    assert_eq!(logs.len(), 3);
    assert_eq!(logs[0]["timestamp_utc"], "2026-02-27T00:00:00.000Z");
    assert_eq!(logs[1]["timestamp_utc"], "2026-02-27T00:30:00.000Z");
    assert_eq!(logs[2]["timestamp_utc"], "2026-02-27T00:45:00.000Z");
}

#[tokio::test]
async fn mcp_tools_call_list_logs_honors_exclude_units() {
    let response = app()
            .oneshot(
                Request::builder()
                    .uri("/mcp")
                    .method("POST")
                    .header(header::CONTENT_TYPE, "application/json")
                    .header(header::AUTHORIZATION, "Bearer token-1234567890ab")
                    .body(Body::from(
                        r#"{"jsonrpc":"2.0","id":312,"method":"tools/call","params":{"name":"list_logs","arguments":{"start_utc":"2026-02-27T00:00:00Z","end_utc":"2026-02-27T01:00:00Z","exclude_units":["ssh.service","cron.service"],"limit":10}}}"#,
                    ))
                    .expect("request build"),
            )
            .await
            .expect("request execution");

    assert_eq!(response.status(), StatusCode::OK);
    let body = response
        .into_body()
        .collect()
        .await
        .expect("collect body")
        .to_bytes();
    let body_json: serde_json::Value = serde_json::from_slice(&body).expect("valid json response");

    assert_eq!(body_json["jsonrpc"], "2.0");
    assert_eq!(body_json["id"], 312);
    let logs = body_json["result"]["structuredContent"]["logs"]
        .as_array()
        .expect("logs array");
    assert_eq!(logs.len(), 1);
    assert_eq!(logs[0]["unit"], "app.service");
}

#[tokio::test]
async fn mcp_resources_read_returns_contents_only() {
    let response = app()
            .oneshot(
                Request::builder()
                    .uri("/mcp")
                    .method("POST")
                    .header(header::CONTENT_TYPE, "application/json")
                    .header(header::AUTHORIZATION, "Bearer token-1234567890ab")
                    .body(Body::from(
                        r#"{"jsonrpc":"2.0","id":4,"method":"resources/read","params":{"uri":"resource://services/snapshot"}}"#,
                    ))
                    .expect("request build"),
            )
            .await
            .expect("request execution");

    assert_eq!(response.status(), StatusCode::OK);
    let body = response
        .into_body()
        .collect()
        .await
        .expect("collect body")
        .to_bytes();
    let body_json: serde_json::Value = serde_json::from_slice(&body).expect("valid json response");

    assert_eq!(body_json["jsonrpc"], "2.0");
    assert_eq!(body_json["id"], 4);
    assert!(body_json["result"]["contents"].is_array());
    assert!(body_json["result"].get("structuredContent").is_none());
}

#[tokio::test]
async fn mcp_resources_read_failed_services_returns_failed_only() {
    let response = app()
            .oneshot(
                Request::builder()
                    .uri("/mcp")
                    .method("POST")
                    .header(header::CONTENT_TYPE, "application/json")
                    .header(header::AUTHORIZATION, "Bearer token-1234567890ab")
                    .body(Body::from(
                        r#"{"jsonrpc":"2.0","id":43,"method":"resources/read","params":{"uri":"resource://services/failed"}}"#,
                    ))
                    .expect("request build"),
            )
            .await
            .expect("request execution");

    assert_eq!(response.status(), StatusCode::OK);
    let body = response
        .into_body()
        .collect()
        .await
        .expect("collect body")
        .to_bytes();
    let body_json: serde_json::Value = serde_json::from_slice(&body).expect("valid json response");

    assert_eq!(body_json["jsonrpc"], "2.0");
    assert_eq!(body_json["id"], 43);
    assert_eq!(
        body_json["result"]["contents"][0]["uri"],
        "resource://services/failed"
    );
    let content_text = body_json["result"]["contents"][0]["text"]
        .as_str()
        .expect("text content");
    let content_json: serde_json::Value =
        serde_json::from_str(content_text).expect("valid resource json");
    assert_eq!(content_json["services"].as_array().map(Vec::len), Some(1));
    assert_eq!(content_json["services"][0]["unit"], "b.service");
    assert_eq!(content_json["services"][0]["active_state"], "failed");
}

#[tokio::test]
async fn mcp_resources_list_includes_fixed_uris() {
    let response = app()
        .oneshot(
            Request::builder()
                .uri("/mcp")
                .method("POST")
                .header(header::CONTENT_TYPE, "application/json")
                .header(header::AUTHORIZATION, "Bearer token-1234567890ab")
                .body(Body::from(
                    r#"{"jsonrpc":"2.0","id":41,"method":"resources/list","params":{}}"#,
                ))
                .expect("request build"),
        )
        .await
        .expect("request execution");

    assert_eq!(response.status(), StatusCode::OK);
    let body = response
        .into_body()
        .collect()
        .await
        .expect("collect body")
        .to_bytes();
    let body_json: serde_json::Value = serde_json::from_slice(&body).expect("valid json response");

    assert_eq!(body_json["jsonrpc"], "2.0");
    assert_eq!(body_json["id"], 41);
    assert!(body_json["result"]["resources"].is_array());
    assert_eq!(
        body_json["result"]["resources"][0]["uri"],
        "resource://services/snapshot"
    );
    assert_eq!(
        body_json["result"]["resources"][1]["uri"],
        "resource://services/failed"
    );
    assert_eq!(
        body_json["result"]["resources"][2]["uri"],
        "resource://logs/recent"
    );
}

#[tokio::test]
async fn mcp_tools_call_list_logs_invalid_limit_returns_invalid_params() {
    let response = app()
            .oneshot(
                Request::builder()
                    .uri("/mcp")
                    .method("POST")
                    .header(header::CONTENT_TYPE, "application/json")
                    .header(header::AUTHORIZATION, "Bearer token-1234567890ab")
                    .body(Body::from(
                        r#"{"jsonrpc":"2.0","id":42,"method":"tools/call","params":{"name":"list_logs","arguments":{"start_utc":"2026-02-27T00:00:00Z","end_utc":"2026-02-27T01:00:00Z","limit":1001}}}"#,
                    ))
                    .expect("request build"),
            )
            .await
            .expect("request execution");

    assert_eq!(response.status(), StatusCode::OK);
    let body = response
        .into_body()
        .collect()
        .await
        .expect("collect body")
        .to_bytes();
    let body_json: serde_json::Value = serde_json::from_slice(&body).expect("valid json response");

    assert_eq!(body_json["jsonrpc"], "2.0");
    assert_eq!(body_json["id"], 42);
    assert_eq!(body_json["error"]["code"], -32602);
    assert_eq!(body_json["error"]["data"]["code"], "invalid_limit");
}

#[tokio::test]
async fn mcp_notification_returns_no_content() {
    let response = app()
        .oneshot(
            Request::builder()
                .uri("/mcp")
                .method("POST")
                .header(header::CONTENT_TYPE, "application/json")
                .header(header::AUTHORIZATION, "Bearer token-1234567890ab")
                .body(Body::from(r#"{"jsonrpc":"2.0","method":"ping"}"#))
                .expect("request build"),
        )
        .await
        .expect("request execution");

    assert_eq!(response.status(), StatusCode::NO_CONTENT);
    let body = response
        .into_body()
        .collect()
        .await
        .expect("collect body")
        .to_bytes();
    assert!(body.is_empty());
}

#[tokio::test]
async fn mcp_batch_notifications_return_no_content() {
    let response = app()
            .oneshot(
                Request::builder()
                    .uri("/mcp")
                    .method("POST")
                    .header(header::CONTENT_TYPE, "application/json")
                    .header(header::AUTHORIZATION, "Bearer token-1234567890ab")
                    .body(Body::from(
                        r#"[{"jsonrpc":"2.0","method":"ping"},{"jsonrpc":"2.0","method":"tools/list","params":{}}]"#,
                    ))
                    .expect("request build"),
            )
            .await
            .expect("request execution");

    assert_eq!(response.status(), StatusCode::NO_CONTENT);
    let body = response
        .into_body()
        .collect()
        .await
        .expect("collect body")
        .to_bytes();
    assert!(body.is_empty());
}

#[tokio::test]
async fn mcp_batch_mixed_requests_return_only_id_responses() {
    let response = app()
            .oneshot(
                Request::builder()
                    .uri("/mcp")
                    .method("POST")
                    .header(header::CONTENT_TYPE, "application/json")
                    .header(header::AUTHORIZATION, "Bearer token-1234567890ab")
                    .body(Body::from(
                        r#"[{"jsonrpc":"2.0","method":"ping"},{"jsonrpc":"2.0","id":100,"method":"ping"},{"jsonrpc":"2.0","id":200,"method":"tools/list","params":{}}]"#,
                    ))
                    .expect("request build"),
            )
            .await
            .expect("request execution");

    assert_eq!(response.status(), StatusCode::OK);
    let body = response
        .into_body()
        .collect()
        .await
        .expect("collect body")
        .to_bytes();
    let body_json: serde_json::Value = serde_json::from_slice(&body).expect("valid json response");

    assert!(body_json.is_array());
    let responses = body_json.as_array().expect("batch response array");
    assert_eq!(responses.len(), 2);
    let ids: Vec<i64> = responses
        .iter()
        .filter_map(|item| item["id"].as_i64())
        .collect();
    assert!(ids.contains(&100));
    assert!(ids.contains(&200));
}

#[tokio::test]
async fn mcp_resources_read_unknown_uri_returns_resource_not_found_data() {
    let response = app()
            .oneshot(
                Request::builder()
                    .uri("/mcp")
                    .method("POST")
                    .header(header::CONTENT_TYPE, "application/json")
                    .header(header::AUTHORIZATION, "Bearer token-1234567890ab")
                    .body(Body::from(
                        r#"{"jsonrpc":"2.0","id":501,"method":"resources/read","params":{"uri":"resource://unknown/item"}}"#,
                    ))
                    .expect("request build"),
            )
            .await
            .expect("request execution");

    assert_eq!(response.status(), StatusCode::OK);
    let body = response
        .into_body()
        .collect()
        .await
        .expect("collect body")
        .to_bytes();
    let body_json: serde_json::Value = serde_json::from_slice(&body).expect("valid json response");

    assert_eq!(body_json["jsonrpc"], "2.0");
    assert_eq!(body_json["id"], 501);
    assert_eq!(body_json["error"]["code"], -32601);
    assert_eq!(body_json["error"]["data"]["code"], "resource_not_found");
}

#[tokio::test]
async fn mcp_tools_call_unknown_tool_returns_tool_not_found_data() {
    let response = app()
            .oneshot(
                Request::builder()
                    .uri("/mcp")
                    .method("POST")
                    .header(header::CONTENT_TYPE, "application/json")
                    .header(header::AUTHORIZATION, "Bearer token-1234567890ab")
                    .body(Body::from(
                        r#"{"jsonrpc":"2.0","id":503,"method":"tools/call","params":{"name":"unknown_tool","arguments":{}}}"#,
                    ))
                    .expect("request build"),
            )
            .await
            .expect("request execution");

    assert_eq!(response.status(), StatusCode::OK);
    let body = response
        .into_body()
        .collect()
        .await
        .expect("collect body")
        .to_bytes();
    let body_json: serde_json::Value = serde_json::from_slice(&body).expect("valid json response");

    assert_eq!(body_json["jsonrpc"], "2.0");
    assert_eq!(body_json["id"], 503);
    assert_eq!(body_json["error"]["code"], -32601);
    assert_eq!(body_json["error"]["data"]["code"], "tool_not_found");
}

#[tokio::test]
async fn mcp_tools_call_malformed_params_returns_invalid_params() {
    let response = app()
            .oneshot(
                Request::builder()
                    .uri("/mcp")
                    .method("POST")
                    .header(header::CONTENT_TYPE, "application/json")
                    .header(header::AUTHORIZATION, "Bearer token-1234567890ab")
                    .body(Body::from(
                        r#"{"jsonrpc":"2.0","id":502,"method":"tools/call","params":{"name":"list_logs","arguments":"not-an-object"}}"#,
                    ))
                    .expect("request build"),
            )
            .await
            .expect("request execution");

    assert_eq!(response.status(), StatusCode::OK);
    let body = response
        .into_body()
        .collect()
        .await
        .expect("collect body")
        .to_bytes();
    let body_json: serde_json::Value = serde_json::from_slice(&body).expect("valid json response");

    assert_eq!(body_json["jsonrpc"], "2.0");
    assert_eq!(body_json["id"], 502);
    assert_eq!(body_json["error"]["code"], -32602);
}

#[tokio::test]
async fn mcp_parse_error_for_invalid_json() {
    let response = app()
        .oneshot(
            Request::builder()
                .uri("/mcp")
                .method("POST")
                .header(header::CONTENT_TYPE, "application/json")
                .header(header::AUTHORIZATION, "Bearer token-1234567890ab")
                .body(Body::from("{"))
                .expect("request build"),
        )
        .await
        .expect("request execution");

    assert_eq!(response.status(), StatusCode::OK);
}
