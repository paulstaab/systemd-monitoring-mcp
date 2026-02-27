use std::sync::Arc;

use systemd_monitoring_mcp::{
    AppState, build_app, config::Config, logging, systemd_client::DbusSystemdClient,
};
use tracing::{info, warn};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    logging::init_logging();

    let config = Config::from_env()?;
    if !libsystemd::daemon::booted() {
        warn!("systemd not detected; /units calls may fail");
    }

    let provider = Arc::new(DbusSystemdClient::new());
    let state = AppState::new(config.api_token.clone(), provider);
    let app = build_app(state);
    let bind_socket = config.bind_socket()?;
    let listener = tokio::net::TcpListener::bind(bind_socket).await?;

    info!(
        bind_addr = %config.bind_addr,
        bind_port = config.bind_port,
        "server starting"
    );

    axum::serve(listener, app).await?;
    Ok(())
}
