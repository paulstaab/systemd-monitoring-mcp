use std::sync::Arc;

use axum::{
    middleware,
    routing::{get, post},
    Router,
};
use ipnet::IpNet;

pub mod api;
pub mod auth;
pub mod config;
pub mod errors;
pub mod logging;
pub mod systemd_client;

use systemd_client::UnitProvider;

#[derive(Clone)]
pub struct AppState {
    pub api_token: Arc<str>,
    pub allowed_cidr: Option<IpNet>,
    pub unit_provider: Arc<dyn UnitProvider>,
}

impl AppState {
    pub fn new(
        api_token: String,
        allowed_cidr: Option<IpNet>,
        unit_provider: Arc<dyn UnitProvider>,
    ) -> Self {
        Self {
            api_token: Arc::<str>::from(api_token),
            allowed_cidr,
            unit_provider,
        }
    }
}

pub fn build_app(state: AppState) -> Router {
    let protected = Router::new()
        .route("/services", get(api::list_services))
        .route("/logs", get(api::list_logs))
        .route_layer(middleware::from_fn_with_state(
            state.clone(),
            auth::require_bearer_token,
        ));

    Router::new()
        .route("/", get(api::discovery).post(api::mcp_endpoint))
        .route("/health", get(api::health))
        .route("/.well-known/mcp", get(api::discovery))
        .route("/mcp", post(api::mcp_endpoint))
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
        let state = AppState::new("token-1".to_string(), None, Arc::new(MockProvider));
        build_app(state)
    }

    fn app_with_allowed_cidr(cidr: &str) -> Router {
        let state = AppState::new(
            "token-1".to_string(),
            Some(cidr.parse().expect("valid cidr")),
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
    async fn services_require_token() {
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

        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn logs_require_token() {
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

        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn logs_with_token_succeeds() {
        let response = app()
            .oneshot(
                Request::builder()
                    .uri(
                        "/logs?start_utc=2026-02-27T00:00:00Z&end_utc=2026-02-27T01:00:00Z&limit=1",
                    )
                    .method("GET")
                    .header(header::AUTHORIZATION, "Bearer token-1")
                    .body(Body::empty())
                    .expect("request build"),
            )
            .await
            .expect("request execution");

        assert_eq!(response.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn services_with_token_succeeds() {
        let response = app()
            .oneshot(
                Request::builder()
                    .uri("/services")
                    .method("GET")
                    .header(header::AUTHORIZATION, "Bearer token-1")
                    .body(Body::empty())
                    .expect("request build"),
            )
            .await
            .expect("request execution");

        assert_eq!(response.status(), StatusCode::OK);
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
        assert_eq!(body_json["services_endpoint"], "/services");
        assert_eq!(body_json["logs_endpoint"], "/logs");
    }

    #[tokio::test]
    async fn root_discovery_is_public() {
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

        assert_eq!(response.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn mcp_unknown_method_returns_method_not_found() {
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
                    .body(Body::from(
                        r#"{"jsonrpc":"2.0","id":1,"method":"initialize"}"#,
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
        assert!(body_json["result"]["capabilities"]["prompts"].is_object());
        assert_eq!(
            body_json["result"]["metadata"]["restEndpoints"]["services"],
            "/services"
        );
        assert_eq!(
            body_json["result"]["metadata"]["restEndpoints"]["logs"],
            "/logs"
        );
    }

    #[tokio::test]
    async fn root_mcp_initialize_returns_result() {
        let response = app()
            .oneshot(
                Request::builder()
                    .uri("/")
                    .method("POST")
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(
                        r#"{"jsonrpc":"2.0","id":1,"method":"initialize"}"#,
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
    }

    #[tokio::test]
    async fn mcp_parse_error_for_invalid_json() {
        let response = app()
            .oneshot(
                Request::builder()
                    .uri("/mcp")
                    .method("POST")
                    .header(header::CONTENT_TYPE, "application/json")
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
