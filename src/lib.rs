use std::sync::Arc;

use axum::{Router, middleware, routing::get};

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
    pub unit_provider: Arc<dyn UnitProvider>,
}

impl AppState {
    pub fn new(api_token: String, unit_provider: Arc<dyn UnitProvider>) -> Self {
        Self {
            api_token: Arc::<str>::from(api_token),
            unit_provider,
        }
    }
}

pub fn build_app(state: AppState) -> Router {
    let protected = Router::new()
        .route("/units", get(api::list_units))
        .route_layer(middleware::from_fn_with_state(
            state.clone(),
            auth::require_bearer_token,
        ));

    Router::new()
        .route("/health", get(api::health))
        .merge(protected)
        .layer(middleware::from_fn(logging::request_logging_middleware))
        .with_state(state)
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use axum::{
        body::Body,
        http::{Request, StatusCode, header},
    };
    use http_body_util::BodyExt;
    use tower::ServiceExt;

    use crate::systemd_client::{UnitProvider, UnitStatus};

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
    }

    fn app() -> Router {
        let state = AppState::new("token-1".to_string(), Arc::new(MockProvider));
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
    async fn units_require_token() {
        let response = app()
            .oneshot(
                Request::builder()
                    .uri("/units")
                    .method("GET")
                    .body(Body::empty())
                    .expect("request build"),
            )
            .await
            .expect("request execution");

        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn units_with_token_succeeds() {
        let response = app()
            .oneshot(
                Request::builder()
                    .uri("/units")
                    .method("GET")
                    .header(header::AUTHORIZATION, "Bearer token-1")
                    .body(Body::empty())
                    .expect("request build"),
            )
            .await
            .expect("request execution");

        assert_eq!(response.status(), StatusCode::OK);
    }
}
