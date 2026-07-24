use std::sync::Arc;

use axum::{
    middleware,
    routing::{get, post},
    Router,
};

pub mod auth;
pub mod config;
pub mod domain;
pub mod errors;
pub mod http;
pub mod logging;
pub mod mcp;
pub mod podman;
pub mod systemd_client;

use podman::{CliPodmanProvider, PodmanProvider};
use systemd_client::UnitProvider;

#[derive(Clone)]
pub struct AppState {
    pub api_token: Arc<str>,
    pub unit_provider: Arc<dyn UnitProvider>,
    pub podman_provider: Arc<dyn PodmanProvider>,
}

impl AppState {
    /// Creates shared application state used by Axum handlers and middleware.
    pub fn new(api_token: String, unit_provider: Arc<dyn UnitProvider>) -> Self {
        Self {
            api_token: Arc::from(api_token),
            unit_provider,
            podman_provider: Arc::new(CliPodmanProvider),
        }
    }

    /// Replaces the Podman adapter, primarily for deterministic tests.
    pub fn with_podman_provider(mut self, podman_provider: Arc<dyn PodmanProvider>) -> Self {
        self.podman_provider = podman_provider;
        self
    }
}

/// Builds the HTTP router with public and authenticated MCP routes.
///
/// The `/mcp` and systemd status routes are protected by bearer auth
/// middleware. Health and discovery routes remain public because they expose
/// only basic liveness and package metadata.
pub fn build_app(state: AppState) -> Router {
    let protected = Router::new()
        .route("/mcp", post(http::handlers::mcp_endpoint))
        .route(
            "/systemd/system/status",
            get(http::handlers::systemd_system_status),
        )
        .route(
            "/systemd/user/status",
            get(http::handlers::systemd_user_status),
        )
        .route_layer(middleware::from_fn_with_state(
            state.clone(),
            auth::require_bearer_token,
        ));

    Router::new()
        .route("/health", get(http::handlers::health))
        .route("/.well-known/mcp", get(http::handlers::discovery))
        .merge(protected)
        .layer(middleware::from_fn(logging::request_logging_middleware))
        .with_state(state)
}

#[cfg(test)]
mod tests;
