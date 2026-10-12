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

    axum::serve(listener, app).await?;
    Ok(())
}
