use std::sync::Arc;

use systemd_monitoring_mcp::{
    build_app,
    config::Config,
    logging,
    systemd_client::{ensure_systemd_available, DbusSystemdClient},
    AppState,
};
use tracing::info;

#[tokio::main]
/// Process entrypoint: validates configuration and systemd, builds shared state,
/// and starts the rate-limited Axum server.
///
/// The configured rate and burst are injected into the one process-wide bucket
/// and logged without exposing the bearer token.
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    logging::init_logging();

    let config = Config::from_env()?;
    ensure_systemd_available().await?;

    let provider = Arc::new(DbusSystemdClient::new());
    let bind_socket = config.bind_socket()?;
    let state = AppState::new_with_rate_limit(
        config.api_token.clone(),
        provider,
        config.rate_limit_policy(),
    );
    let app = build_app(state);
    let listener = tokio::net::TcpListener::bind(bind_socket).await?;

    info!(
        bind_addr = %config.bind_addr,
        bind_port = config.bind_port,
        rate_limit_requests_per_second = config.rate_limit_requests_per_second,
        rate_limit_burst = config.rate_limit_burst,
        "server starting"
    );

    axum::serve(listener, app.into_make_service()).await?;
    Ok(())
}
