// Boalix Sync Server — Centralized Cloud Tracking API
// Hosts tracking pixel, click redirects, unsubscribe pages,
// and accepts event push/pull from Boalix desktop clients.

mod config;
mod db;
mod auth;
mod routes;

use axum::Router;
use std::net::SocketAddr;
use tower_http::cors::{CorsLayer, Any};
use tower_http::trace::TraceLayer;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // Load .env file if present (for local development)
    let _ = dotenvy::dotenv();

    // Initialize tracing
    tracing_subscriber::fmt()
        .with_env_filter(
            std::env::var("RUST_LOG").unwrap_or_else(|_| "boalix_sync_server=debug,warn".into()),
        )
        .init();

    tracing::info!("🚀 Boalix Sync Server starting...");

    // Initialize database
    db::init().await?;
    tracing::info!("✅ Database initialized");

    // Build Axum router
    let tracking_routes = routes::tracking::router();
    let sync_routes = routes::sync::router();
    let health_routes = routes::health::router();

    let app = Router::new()
        .merge(tracking_routes)   // Public: /t/:id, /c/:id/:url, /u/:id, /unsub/:id
        .merge(sync_routes)       // Authenticated: /api/events, /api/sync
        .merge(health_routes)     // Public: /health, /ping
        .layer(
            CorsLayer::new()
                .allow_origin(Any)
                .allow_methods(Any)
                .allow_headers(Any),
        )
        .layer(TraceLayer::new_for_http());

    // Bind address
    let port: u16 = std::env::var("PORT")
        .ok()
        .and_then(|p| p.parse().ok())
        .unwrap_or(8080);

    let addr = SocketAddr::from(([0, 0, 0, 0], port));
    tracing::info!("📡 Listening on http://{}", addr);

    let listener = tokio::net::TcpListener::bind(addr).await?;
    axum::serve(listener, app).await?;

    Ok(())
}
