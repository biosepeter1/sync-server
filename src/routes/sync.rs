// routes/sync.rs — Authenticated API endpoints for desktop ↔ cloud synchronization
//
// Desktop app uses these to:
//   1. Push local delivery_events in batches (POST /api/events)
//   2. Register tracking queue entries so the server can resolve tracking IDs (POST /api/queue)
//   3. Pull events it may have missed while offline (GET /api/sync)
//   4. Check/update the active tracking domain (GET/PUT /api/domain)

use axum::{
    extract::{Extension, Query},
    middleware,
    response::Json,
    routing::{get, post, put},
    Router,
};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use crate::auth::{require_api_key, TenantId};

pub fn router() -> Router {
    let authenticated_routes = Router::new()
        // Event batch push — desktop → cloud
        .route("/api/events", post(push_events))
        // Queue registration — desktop tells server about tracking IDs before sending
        .route("/api/queue", post(register_queue_entries))
        // Pull events since a timestamp — cloud → desktop
        .route("/api/sync", get(pull_events))
        // Active tracking domain management
        .route("/api/domain", get(get_active_domain))
        .route("/api/domain", put(update_active_domain))
        // Apply API key auth middleware to all /api/* routes
        .route_layer(middleware::from_fn(require_api_key));

    Router::new()
        .merge(authenticated_routes)
        // Tenant self-registration (for initial setup) - bypasses X-Api-Key auth
        .route("/api/register", post(register_tenant))
}

// ── Structs ───────────────────────────────────────────────────────────────────

#[derive(Deserialize, Serialize)]
pub struct EventRecord {
    pub id: String,
    pub campaign_id: String,
    pub subscriber_id: String,
    pub event_type: String,
    pub ab_variant: Option<String>,
    pub metadata: Option<Value>,
    pub occurred_at: Option<String>,
}

#[derive(Deserialize, Serialize)]
pub struct QueueEntry {
    pub id: String,
    pub campaign_id: String,
    pub subscriber_id: String,
    pub to_email: String,
    pub tracking_id: String,
    pub ab_variant: Option<String>,
}

#[derive(Deserialize)]
pub struct PullQuery {
    pub since: Option<String>, // ISO 8601 datetime
    pub limit: Option<u32>,
}

#[derive(Deserialize)]
pub struct DomainUpdate {
    pub domain: String,
}

#[derive(Deserialize)]
pub struct TenantRegistration {
    pub name: String,
    pub secret: String, // must match server's API_SECRET env var
}

// ── Handlers ──────────────────────────────────────────────────────────────────

/// POST /api/events
/// Desktop pushes a batch of delivery_events.
/// Idempotent — uses INSERT OR IGNORE so duplicate pushes are safe.
async fn push_events(
    Extension(TenantId(tenant_id)): Extension<TenantId>,
    Json(events): Json<Vec<EventRecord>>,
) -> Json<Value> {
    let total = events.len();
    let mut inserted = 0usize;

    for ev in &events {
        let meta = ev.metadata.as_ref()
            .map(|m| m.to_string())
            .unwrap_or_else(|| "{}".into());
        let variant = ev.ab_variant.as_deref().unwrap_or("A");

        let result = crate::db::log_event(
            &ev.id,
            &ev.campaign_id,
            &ev.subscriber_id,
            &tenant_id,
            &ev.event_type,
            variant,
            &meta,
            Some("desktop"),
        );

        if result.is_ok() {
            inserted += 1;
        }
    }

    tracing::info!(
        "📥 Event push: tenant={} total={} inserted={}",
        tenant_id, total, inserted
    );

    Json(json!({
        "ok": true,
        "received": total,
        "inserted": inserted,
        "duplicate_skipped": total - inserted,
    }))
}

/// POST /api/queue
/// Desktop registers delivery_queue rows before sending a campaign.
/// This allows the sync server to resolve tracking IDs → campaign/subscriber.
async fn register_queue_entries(
    Extension(TenantId(tenant_id)): Extension<TenantId>,
    Json(entries): Json<Vec<QueueEntry>>,
) -> Json<Value> {
    let total = entries.len();
    let mut inserted = 0usize;

    for entry in &entries {
        let variant = entry.ab_variant.as_deref().unwrap_or("A");
        let result = crate::db::with_db(|conn| {
            conn.execute(
                "INSERT OR IGNORE INTO delivery_queue
                    (id, campaign_id, subscriber_id, tenant_id, to_email, tracking_id, ab_variant)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
                rusqlite::params![
                    entry.id,
                    entry.campaign_id,
                    entry.subscriber_id,
                    tenant_id,
                    entry.to_email,
                    entry.tracking_id,
                    variant,
                ],
            )?;
            Ok(())
        });
        if result.is_ok() {
            inserted += 1;
        }
    }

    tracing::info!(
        "📤 Queue registered: tenant={} total={} inserted={}",
        tenant_id, total, inserted
    );

    Json(json!({
        "ok": true,
        "registered": inserted,
    }))
}

/// GET /api/sync?since=<datetime>&limit=<n>
/// Desktop pulls events that happened on the cloud (while desktop was offline).
async fn pull_events(
    Extension(TenantId(tenant_id)): Extension<TenantId>,
    Query(q): Query<PullQuery>,
) -> Json<Value> {
    let since = q.since.as_deref().unwrap_or("1970-01-01T00:00:00");
    let limit = q.limit.unwrap_or(500).min(2000) as i64;

    let events = crate::db::with_db(|conn| {
        let mut stmt = conn.prepare(
            "SELECT id, campaign_id, subscriber_id, event_type, ab_variant, metadata, occurred_at
             FROM delivery_events
             WHERE tenant_id = ?1
               AND occurred_at > datetime(?2)
               AND synced_from = 'direct'
             ORDER BY occurred_at ASC
             LIMIT ?3"
        )?;
        let rows = stmt.query_map(
            rusqlite::params![tenant_id, since, limit],
            |row| Ok(json!({
                "id":            row.get::<_, String>(0)?,
                "campaign_id":   row.get::<_, String>(1)?,
                "subscriber_id": row.get::<_, String>(2)?,
                "event_type":    row.get::<_, String>(3)?,
                "ab_variant":    row.get::<_, String>(4)?,
                "metadata":      row.get::<_, String>(5)?,
                "occurred_at":   row.get::<_, String>(6)?,
            })),
        )?;
        let events: Vec<Value> = rows.flatten().collect();
        Ok(events)
    }).unwrap_or_default();

    Json(json!({
        "ok": true,
        "events": events,
        "count": events.len(),
    }))
}

/// GET /api/domain
/// Returns the currently active tracking domain for this tenant.
/// Desktop uses this to detect if the domain changed server-side.
async fn get_active_domain(
    Extension(TenantId(_tenant_id)): Extension<TenantId>,
) -> Json<Value> {
    let domain = crate::config::tracking_domain();
    Json(json!({
        "ok": true,
        "domain": domain,
    }))
}

/// PUT /api/domain
/// Allows a tenant admin to update the tracking domain used in future emails.
/// NOTE: This only updates what the server reports — the desktop still needs
/// to update its own settings to start embedding the new URL in emails.
async fn update_active_domain(
    Extension(TenantId(tenant_id)): Extension<TenantId>,
    Json(body): Json<DomainUpdate>,
) -> Json<Value> {
    tracing::warn!(
        "⚠️  Domain update requested by tenant={}: {}",
        tenant_id, body.domain
    );
    // In a real deployment you'd write to DB or update env config.
    // For now we return the requested domain as an ack.
    Json(json!({
        "ok": true,
        "domain": body.domain,
        "note": "Update your TRACKING_DOMAIN env var on the server and restart to make this permanent.",
    }))
}

/// POST /api/register  (uses master API_SECRET for auth, not tenant key)
/// Creates a new tenant account during SaaS onboarding.
/// The secret in the request body must match the server's API_SECRET env var.
async fn register_tenant(
    Json(body): Json<TenantRegistration>,
) -> Json<Value> {
    if body.secret != crate::config::api_secret() {
        return Json(json!({ "ok": false, "error": "Invalid secret" }));
    }

    let tenant_id = uuid::Uuid::new_v4().to_string();
    let api_key = format!("bx_{}", uuid::Uuid::new_v4().to_string().replace('-', ""));

    let result = crate::db::with_db(|conn| {
        conn.execute(
            "INSERT INTO tenants (id, name, api_key) VALUES (?1, ?2, ?3)",
            rusqlite::params![tenant_id, body.name, api_key],
        )?;
        Ok(())
    });

    match result {
        Ok(_) => {
            tracing::info!("✅ New tenant registered: {} ({})", body.name, tenant_id);
            Json(json!({
                "ok": true,
                "tenant_id": tenant_id,
                "api_key": api_key,
                "note": "Save this api_key — it will not be shown again.",
            }))
        }
        Err(e) => Json(json!({ "ok": false, "error": e.to_string() })),
    }
}
