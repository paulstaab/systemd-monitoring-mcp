use std::time::Instant;

use axum::{extract::Request, middleware::Next, response::Response};
use tracing::{info, warn};
use tracing_subscriber::{EnvFilter, fmt};

pub fn init_logging() {
    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info"));
    fmt()
        .with_env_filter(filter)
        .with_target(false)
        .compact()
        .init();
}

pub async fn request_logging_middleware(request: Request, next: Next) -> Response {
    let method = request.method().clone();
    let path = request.uri().path().to_string();
    let started_at = Instant::now();

    let response = next.run(request).await;
    let status = response.status();
    let elapsed_ms = started_at.elapsed().as_millis();

    info!(
        method = %method,
        path = %path,
        status = status.as_u16(),
        duration_ms = elapsed_ms,
        "request summary"
    );

    if status.as_u16() == 401 {
        warn!(method = %method, path = %path, "authentication failure");
    }

    response
}
