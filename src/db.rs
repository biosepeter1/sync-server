// db.rs — SQLite database layer for the sync server

use once_cell::sync::OnceCell;
use rusqlite::{Connection, params};
use std::sync::Mutex;

pub static DB: OnceCell<Mutex<Connection>> = OnceCell::new();

pub async fn init() -> anyhow::Result<()> {
    let path = crate::config::database_path();
    let conn = Connection::open(&path)?;

    conn.execute_batch(
        "PRAGMA journal_mode=WAL; PRAGMA synchronous=NORMAL; PRAGMA foreign_keys=ON;"
    )?;

    conn.execute_batch(SCHEMA)?;

    // Migrations for schema evolution
    let _ = conn.execute("ALTER TABLE delivery_events ADD COLUMN synced_from TEXT", []);
    let _ = conn.execute("ALTER TABLE delivery_events ADD COLUMN metadata TEXT DEFAULT '{}'", []);

    DB.set(Mutex::new(conn))
        .map_err(|_| anyhow::anyhow!("DB already initialized"))?;

    Ok(())
}

pub fn with_db<F, T>(f: F) -> anyhow::Result<T>
where
    F: FnOnce(&mut Connection) -> anyhow::Result<T>,
{
    let mut guard = DB
        .get()
        .ok_or_else(|| anyhow::anyhow!("Database not initialized"))?
        .lock()
        .map_err(|_| anyhow::anyhow!("DB mutex poisoned"))?;
    f(&mut *guard)
}

/// Log a tracking event to the server-side database.
/// Called from tracking routes when a pixel is loaded or link is clicked.
pub fn log_event(
    event_id: &str,
    campaign_id: &str,
    subscriber_id: &str,
    tenant_id: &str,
    event_type: &str,
    ab_variant: &str,
    metadata_json: &str,
    synced_from: Option<&str>,
) -> anyhow::Result<()> {
    with_db(|conn| {
        conn.execute(
            "INSERT OR IGNORE INTO delivery_events
                (id, campaign_id, subscriber_id, tenant_id, event_type, ab_variant, metadata, synced_from, occurred_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, datetime('now'))",
            params![
                event_id,
                campaign_id,
                subscriber_id,
                tenant_id,
                event_type,
                ab_variant,
                metadata_json,
                synced_from,
            ],
        )?;
        Ok(())
    })
}

/// Look up delivery_queue metadata by tracking_id.
/// Returns (campaign_id, subscriber_id, ab_variant, tenant_id).
pub fn lookup_tracking(tracking_id: &str) -> Option<(String, String, String, String)> {
    with_db(|conn| {
        let row = conn.query_row(
            "SELECT campaign_id, subscriber_id, ab_variant, tenant_id FROM delivery_queue WHERE tracking_id = ?1",
            params![tracking_id],
            |row| Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, String>(3)?,
            )),
        ).ok();
        Ok(row)
    }).unwrap_or(None)
}

const SCHEMA: &str = r#"
-- Tenants (one per Boalix SaaS customer)
CREATE TABLE IF NOT EXISTS tenants (
    id          TEXT PRIMARY KEY,
    name        TEXT NOT NULL,
    api_key     TEXT UNIQUE NOT NULL,
    plan        TEXT NOT NULL DEFAULT 'free',  -- free | pro | enterprise
    is_active   INTEGER NOT NULL DEFAULT 1,
    created_at  TEXT NOT NULL DEFAULT (datetime('now'))
);

-- Delivery queue (mirrors desktop's delivery_queue so tracking IDs can resolve)
CREATE TABLE IF NOT EXISTS delivery_queue (
    id              TEXT PRIMARY KEY,
    campaign_id     TEXT NOT NULL,
    subscriber_id   TEXT NOT NULL,
    tenant_id       TEXT NOT NULL,
    to_email        TEXT NOT NULL,
    tracking_id     TEXT UNIQUE,
    ab_variant      TEXT DEFAULT 'A',
    created_at      TEXT NOT NULL DEFAULT (datetime('now'))
);

-- All tracking events received by this server
CREATE TABLE IF NOT EXISTS delivery_events (
    id              TEXT PRIMARY KEY,
    campaign_id     TEXT NOT NULL,
    subscriber_id   TEXT NOT NULL,
    tenant_id       TEXT NOT NULL DEFAULT 'unknown',
    event_type      TEXT NOT NULL,
    ab_variant      TEXT DEFAULT 'A',
    metadata        TEXT DEFAULT '{}',
    synced_from     TEXT,   -- 'direct' (email recipient hit this server) or 'desktop' (pushed from app)
    occurred_at     TEXT NOT NULL DEFAULT (datetime('now'))
);

CREATE INDEX IF NOT EXISTS idx_events_campaign ON delivery_events(campaign_id, event_type);
CREATE INDEX IF NOT EXISTS idx_events_tenant ON delivery_events(tenant_id, occurred_at);
CREATE INDEX IF NOT EXISTS idx_queue_tracking ON delivery_queue(tracking_id);

-- Suppression list (global across all tenants, per email)
CREATE TABLE IF NOT EXISTS suppressions (
    email       TEXT PRIMARY KEY,
    tenant_id   TEXT NOT NULL,
    reason      TEXT NOT NULL,
    created_at  TEXT NOT NULL DEFAULT (datetime('now'))
);

-- System settings (e.g. active tracking domain)
CREATE TABLE IF NOT EXISTS settings (
    key         TEXT PRIMARY KEY,
    value       TEXT NOT NULL,
    updated_at  TEXT NOT NULL DEFAULT (datetime('now'))
);
"#;
