// routes/health.rs — Public health check endpoints

use axum::{routing::get, Router, Json};
use serde_json::json;

pub fn router() -> Router {
    Router::new()
        .route("/health", get(health))
        .route("/ping", get(ping))
}

async fn health() -> Json<serde_json::Value> {
    let domain = crate::config::tracking_domain();
    Json(json!({
        "status": "ok",
        "service": "boalix-sync-server",
        "tracking_domain": domain,
    }))
}

async fn ping() -> &'static str {
    "pong"
}
