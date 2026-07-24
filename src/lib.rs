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
pub mod rate_limit;
pub mod systemd_client;

use podman::{CliPodmanProvider, PodmanProvider};
use rate_limit::{RateLimitPolicy, RateLimiter};
use systemd_client::UnitProvider;

#[derive(Clone)]
pub struct AppState {
    pub api_token: Arc<str>,
    pub unit_provider: Arc<dyn UnitProvider>,
    pub podman_provider: Arc<dyn PodmanProvider>,
    pub rate_limiter: Arc<RateLimiter>,
}

impl AppState {
    /// Creates shared application state with the documented default rate limit.
    ///
    /// This convenience constructor keeps tests and embedders independent while
    /// production startup can inject validated values with `new_with_rate_limit`.
    pub fn new(api_token: String, unit_provider: Arc<dyn UnitProvider>) -> Self {
        Self::new_with_rate_limit(api_token, unit_provider, RateLimitPolicy::default())
    }

    /// Creates shared application state with an explicit validated rate policy.
    ///
    /// The resulting limiter begins at full burst capacity and is shared by all
    /// clones of this state, ensuring every router clone consumes one bucket.
    pub fn new_with_rate_limit(
        api_token: String,
        unit_provider: Arc<dyn UnitProvider>,
        rate_limit_policy: RateLimitPolicy,
    ) -> Self {
        Self {
            api_token: Arc::from(api_token),
            unit_provider,
            podman_provider: Arc::new(CliPodmanProvider),
            rate_limiter: Arc::new(RateLimiter::new(rate_limit_policy)),
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
/// The global limiter covers public, protected, invalid, and unmatched requests.
/// It runs inside request-summary logging but before bearer authentication and
/// handlers. Health and discovery remain unauthenticated after admission.
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
        .layer(middleware::from_fn_with_state(
            state.clone(),
            rate_limit::rate_limit_middleware,
        ))
        .layer(middleware::from_fn(logging::request_logging_middleware))
        .with_state(state)
}

#[cfg(test)]
mod tests;
