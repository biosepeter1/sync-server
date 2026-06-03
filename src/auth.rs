// auth.rs — API key authentication middleware for the sync server

use axum::{
    extract::Request,
    http::StatusCode,
    middleware::Next,
    response::Response,
    Json,
};
use serde_json::json;

/// Validates the X-Api-Key header against the database.
/// Returns the tenant_id if valid, or 401 Unauthorized.
pub async fn require_api_key(
    mut req: Request,
    next: Next,
) -> Result<Response, (StatusCode, Json<serde_json::Value>)> {
    let api_key = req
        .headers()
        .get("X-Api-Key")
        .and_then(|v| v.to_str().ok())
        .map(|s| s.to_string());

    let Some(key) = api_key else {
        return Err((
            StatusCode::UNAUTHORIZED,
            Json(json!({ "error": "Missing X-Api-Key header" })),
        ));
    };

    // Look up the tenant by API key
    let tenant = crate::db::with_db(|conn| {
        let row = conn.query_row(
            "SELECT id, name, is_active FROM tenants WHERE api_key = ?1",
            rusqlite::params![key],
            |row| Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, i64>(2)?,
            )),
        ).ok();
        Ok(row)
    }).unwrap_or(None);

    match tenant {
        Some((tenant_id, _name, 1)) => {
            // Attach tenant_id as a request extension for downstream handlers
            req.extensions_mut().insert(TenantId(tenant_id));
            Ok(next.run(req).await)
        }
        Some((_, _, _)) => Err((
            StatusCode::FORBIDDEN,
            Json(json!({ "error": "Tenant account is disabled" })),
        )),
        None => Err((
            StatusCode::UNAUTHORIZED,
            Json(json!({ "error": "Invalid API key" })),
        )),
    }
}

/// Extension type carrying the authenticated tenant ID through request handlers.
#[derive(Clone)]
pub struct TenantId(pub String);
