use anyhow::Result;
use std::net::SocketAddr;
use tracing_subscriber::EnvFilter;

mod app;
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
    let state = AppState::new(cfg).await?;
    let app = app::router(state);

    let addr = SocketAddr::from(([0, 0, 0, 0], 8090));
    let listener = tokio::net::TcpListener::bind(addr).await?;
    tracing::info!("ekoru-image-processor listening on {}", addr);

    axum::serve(listener, app)
        .with_graceful_shutdown(shutdown_signal())
        .await?;
    Ok(())
}

/// Resolves on SIGTERM (`docker stop`) or Ctrl+C. axum then stops accepting
/// connections and waits for the requests in progress before `main` returns.
async fn shutdown_signal() {
    let ctrl_c = async {
        tokio::signal::ctrl_c()
            .await
            .expect("failed to install the Ctrl+C handler");
    };

    #[cfg(unix)]
    let terminate = async {
        tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
            .expect("failed to install the SIGTERM handler")
            .recv()
            .await;
    };

    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();

    tokio::select! {
        _ = ctrl_c => {},
        _ = terminate => {},
    }
    tracing::info!("shutting down: finishing the requests in progress");
}
