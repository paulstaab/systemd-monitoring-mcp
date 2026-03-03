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
pub mod systemd_client;

use systemd_client::UnitProvider;

#[derive(Clone)]
pub struct AppState {
    pub api_token: Arc<str>,
    pub unit_provider: Arc<dyn UnitProvider>,
}

impl AppState {
    /// Creates shared application state used by Axum handlers and middleware.
    pub fn new(api_token: String, unit_provider: Arc<dyn UnitProvider>) -> Self {
        Self {
            api_token: Arc::from(api_token),
            unit_provider,
        }
    }
}

/// Builds the HTTP router with public and authenticated MCP routes.
///
/// The `/mcp` route is protected by bearer auth middleware; health and discovery
/// routes remain public.
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
        .layer(middleware::from_fn(logging::request_logging_middleware))
        .with_state(state)
}

#[cfg(test)]
mod tests;
