use std::sync::Arc;

use axum::{
    middleware,
    routing::{get, post},
    Router,
};
use ipnet::IpNet;

pub mod auth;
pub mod config;
pub mod domain;
pub mod errors;
pub mod http;
pub mod logging;
pub mod mcp;
pub mod systemd_client;

use systemd_client::UnitProvider;

#[derive(Clone)]
pub struct AppState {
    pub api_token: Arc<str>,
    pub allowed_cidr: Option<IpNet>,
    pub trusted_proxies: Arc<[IpNet]>,
    pub unit_provider: Arc<dyn UnitProvider>,
}

impl AppState {
    pub fn new(
        api_token: String,
        allowed_cidr: Option<IpNet>,
        trusted_proxies: Vec<IpNet>,
        unit_provider: Arc<dyn UnitProvider>,
    ) -> Self {
        Self {
            api_token: Arc::<str>::from(api_token),
            allowed_cidr,
            trusted_proxies: Arc::from(trusted_proxies),
            unit_provider,
        }
    }
}

pub fn build_app(state: AppState) -> Router {
    let protected = Router::new()
        .route("/mcp", post(http::handlers::mcp_endpoint))
        .route_layer(middleware::from_fn_with_state(
            state.clone(),
            auth::require_bearer_token,
        ));

    Router::new()
        .route("/health", get(http::handlers::health))
        .route("/.well-known/mcp", get(http::handlers::discovery))
        .merge(protected)
        .layer(middleware::from_fn_with_state(
            state.clone(),
            auth::enforce_ip_allowlist,
        ))
        .layer(middleware::from_fn(logging::request_logging_middleware))
        .with_state(state)
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use axum::{
        body::Body,
        extract::connect_info::ConnectInfo,
        http::{header, Request, StatusCode},
    };
    use http_body_util::BodyExt;
    use tower::ServiceExt;

    use crate::systemd_client::{JournalLogEntry, LogQuery, UnitProvider, UnitStatus};

    use super::*;

    struct MockProvider;

    #[async_trait::async_trait]
    impl UnitProvider for MockProvider {
        async fn list_service_units(&self) -> Result<Vec<UnitStatus>, crate::errors::AppError> {
            Ok(vec![
                UnitStatus {
                    name: "z.service".to_string(),
                    state: "active".to_string(),
                    description: None,
                },
                UnitStatus {
                    name: "a.service".to_string(),
                    state: "inactive".to_string(),
                    description: Some("A service".to_string()),
                },
                UnitStatus {
                    name: "b.service".to_string(),
                    state: "failed".to_string(),
                    description: Some("B service".to_string()),
                },
            ])
        }

        async fn list_journal_logs(
            &self,
            _query: &LogQuery,
        ) -> Result<Vec<JournalLogEntry>, crate::errors::AppError> {
            Ok(vec![JournalLogEntry {
                timestamp_utc: "2026-02-27T00:00:00.000Z".to_string(),
                timestamp_unix_usec: 1_772_150_400_000_000,
                unit: Some("ssh.service".to_string()),
                priority: Some(6),
                message: Some("Started OpenSSH server".to_string()),
            }])
        }
    }

    fn app() -> Router {
        let state = AppState::new(
            "token-1234567890ab".to_string(),
            None,
            vec![],
            Arc::new(MockProvider),
        );
        build_app(state)
    }

    fn app_with_allowed_cidr(cidr: &str) -> Router {
        let state = AppState::new(
            "token-1234567890ab".to_string(),
            Some(cidr.parse().expect("valid cidr")),
            vec![],
            Arc::new(MockProvider),
        );
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
        let body_json: serde_json::Value =
            serde_json::from_slice(&body).expect("valid json response");
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
        let body_json: serde_json::Value =
            serde_json::from_slice(&body).expect("valid json response");

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
        let body_json: serde_json::Value =
            serde_json::from_slice(&body).expect("valid json response");

        assert_eq!(body_json["jsonrpc"], "2.0");
        assert_eq!(body_json["id"], 2);
        assert!(body_json["result"]["tools"].is_array());
        assert_eq!(body_json["result"]["tools"][0]["name"], "list_services");
        assert_eq!(body_json["result"]["tools"][1]["name"], "list_logs");
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
        let body_json: serde_json::Value =
            serde_json::from_slice(&body).expect("valid json response");

        assert_eq!(body_json["jsonrpc"], "2.0");
        assert_eq!(body_json["id"], 3);
        assert!(body_json["result"]["structuredContent"]["services"].is_array());
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
        let body_json: serde_json::Value =
            serde_json::from_slice(&body).expect("valid json response");

        assert_eq!(body_json["jsonrpc"], "2.0");
        assert_eq!(body_json["id"], 32);
        assert_eq!(
            body_json["result"]["structuredContent"]["services"]
                .as_array()
                .map(Vec::len),
            Some(1)
        );
        assert_eq!(
            body_json["result"]["structuredContent"]["services"][0]["name"],
            "a.service"
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
        let body_json: serde_json::Value =
            serde_json::from_slice(&body).expect("valid json response");

        assert_eq!(body_json["jsonrpc"], "2.0");
        assert_eq!(body_json["id"], 33);
        assert_eq!(body_json["error"]["code"], -32602);
        assert_eq!(body_json["error"]["data"]["code"], "invalid_state");
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
        let body_json: serde_json::Value =
            serde_json::from_slice(&body).expect("valid json response");

        assert_eq!(body_json["jsonrpc"], "2.0");
        assert_eq!(body_json["id"], 31);
        assert!(body_json["result"]["structuredContent"]["logs"].is_array());
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
        let body_json: serde_json::Value =
            serde_json::from_slice(&body).expect("valid json response");

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
        let body_json: serde_json::Value =
            serde_json::from_slice(&body).expect("valid json response");

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
        assert_eq!(content_json["services"][0]["name"], "b.service");
        assert_eq!(content_json["services"][0]["state"], "failed");
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
        let body_json: serde_json::Value =
            serde_json::from_slice(&body).expect("valid json response");

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
        let body_json: serde_json::Value =
            serde_json::from_slice(&body).expect("valid json response");

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
        let body_json: serde_json::Value =
            serde_json::from_slice(&body).expect("valid json response");

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
        let body_json: serde_json::Value =
            serde_json::from_slice(&body).expect("valid json response");

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
        let body_json: serde_json::Value =
            serde_json::from_slice(&body).expect("valid json response");

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
        let body_json: serde_json::Value =
            serde_json::from_slice(&body).expect("valid json response");

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

    #[tokio::test]
    async fn request_outside_allowed_cidr_is_blocked() {
        let response = app_with_allowed_cidr("10.0.0.0/8")
            .oneshot(
                Request::builder()
                    .uri("/health")
                    .method("GET")
                    .extension(ConnectInfo(std::net::SocketAddr::from((
                        [192, 168, 1, 10],
                        9000,
                    ))))
                    .body(Body::empty())
                    .expect("request build"),
            )
            .await
            .expect("request execution");

        assert_eq!(response.status(), StatusCode::FORBIDDEN);
    }

    #[tokio::test]
    async fn request_inside_allowed_cidr_is_permitted() {
        let response = app_with_allowed_cidr("10.0.0.0/8")
            .oneshot(
                Request::builder()
                    .uri("/health")
                    .method("GET")
                    .extension(ConnectInfo(std::net::SocketAddr::from((
                        [10, 1, 2, 3],
                        9000,
                    ))))
                    .body(Body::empty())
                    .expect("request build"),
            )
            .await
            .expect("request execution");

        assert_eq!(response.status(), StatusCode::OK);
    }
}
