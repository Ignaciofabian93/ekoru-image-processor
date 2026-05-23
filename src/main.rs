use anyhow::Result;
use axum::{
    extract::DefaultBodyLimit,
    middleware,
    routing::{delete, get, post},
    Router,
};
use std::net::SocketAddr;
use tower_http::trace::TraceLayer;
use tracing_subscriber::EnvFilter;

mod auth;
mod config;
mod error;
mod handlers;
mod processor;
mod state;
mod storage;

use state::AppState;

#[tokio::main]
async fn main() -> Result<()> {
    dotenvy::dotenv().ok();

    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| EnvFilter::new("info,aws=warn,hyper=warn")),
        )
        .init();

    let cfg = config::Config::from_env()?;
    let body_limit = cfg.max_upload_bytes + 1024 * 1024;
    let state = AppState::new(cfg).await?;

    let protected = Router::new()
        .route("/process", post(handlers::upload::handle_upload))
        .route("/objects/{*key}", delete(handlers::delete::handle_delete))
        .route_layer(middleware::from_fn_with_state(
            state.clone(),
            auth::require_internal_token,
        ));

    let app = Router::new()
        .route("/health", get(|| async { "ok" }))
        .merge(protected)
        .layer(DefaultBodyLimit::max(body_limit))
        .layer(TraceLayer::new_for_http())
        .with_state(state);

    let addr = SocketAddr::from(([0, 0, 0, 0], 8090));
    let listener = tokio::net::TcpListener::bind(addr).await?;
    tracing::info!("ekoru-image-processor listening on {}", addr);

    axum::serve(listener, app).await?;
    Ok(())
}
