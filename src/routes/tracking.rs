// routes/tracking.rs — Public tracking endpoints
// These are the URLs embedded in emails sent by Boalix desktop clients.
// Recipients hit these endpoints when they open emails, click links, or unsubscribe.

use axum::{
    extract::Path,
    extract::Query,
    response::{Html, Redirect, Response},
    routing::get,
    Router,
};
use base64::Engine;
use serde::Deserialize;

#[derive(Deserialize)]
pub struct ClickQuery {
    pub t: Option<String>, // tracking ID
    pub r: Option<String>, // redirect URL (base64 encoded)
}

pub fn router() -> Router {
    Router::new()
        // Open tracking pixel
        .route("/t/:id", get(track_open))
        // Click tracking (new-style path: /c/:id/:b64url)
        .route("/c/:id/:url", get(track_click_path))
        // Click tracking (legacy query-string style: /c?t=:id&r=:b64url)
        .route("/c", get(track_click_query))
        // Unsubscribe page (GET = show page, POST = one-click RFC 8058)
        .route("/u/:id", get(handle_unsubscribe))
        .route("/unsub/:id", get(handle_unsubscribe).post(handle_one_click_unsubscribe))
}

// ── Open Pixel ────────────────────────────────────────────────────────────────

async fn track_open(Path(id): Path<String>) -> Response {
    tracing::info!("📬 Open tracked: {}", id);

    if id.starts_with("test-") {
        let email = decode_test_email(&id);
        tracing::info!("🧪 TEST open for: {}", email);
    } else if let Some((cid, sid, variant, tenant_id)) = crate::db::lookup_tracking(&id) {
        let event_id = format!("open-{}", id);
        crate::db::log_event(
            &event_id, &cid, &sid, &tenant_id,
            "open", &variant, "{}", Some("direct"),
        ).ok();
        tracing::info!("✅ Open logged: campaign={} subscriber={}", cid, sid);
    }

    // 1×1 transparent GIF
    let pixel = base64::engine::general_purpose::STANDARD
        .decode("R0lGODlhAQABAIAAAAAAAP///yH5BAEAAAAALAAAAAABAAEAAAIBRAA7")
        .unwrap();
    Response::builder()
        .header("Content-Type", "image/gif")
        .header("Cache-Control", "no-cache, no-store, must-revalidate")
        .body(axum::body::Body::from(pixel))
        .unwrap()
}

// ── Click Tracking ────────────────────────────────────────────────────────────

async fn track_click_path(Path((id, url_b64)): Path<(String, String)>) -> Redirect {
    let target = decode_b64_url(&url_b64);

    if id.starts_with("test-") {
        let email = decode_test_email(&id);
        tracing::info!("🧪 TEST click for: {} → {}", email, target);
    } else if let Some((cid, sid, variant, tenant_id)) = crate::db::lookup_tracking(&id) {
        let event_id = format!("click-{}", id);
        let meta = serde_json::json!({ "url": target }).to_string();
        crate::db::log_event(
            &event_id, &cid, &sid, &tenant_id,
            "click", &variant, &meta, Some("direct"),
        ).ok();
    }

    Redirect::to(&target)
}

async fn track_click_query(Query(q): Query<ClickQuery>) -> Redirect {
    let target = q.r.as_ref()
        .map(|r| decode_b64_url(r))
        .unwrap_or_else(|| "https://boalix.com".into());

    if let Some(t) = &q.t {
        if t.starts_with("test-") {
            let email = decode_test_email(t);
            tracing::info!("🧪 TEST click for: {}", email);
        } else if let Some((cid, sid, variant, tenant_id)) = crate::db::lookup_tracking(t) {
            let event_id = format!("click-{}", t);
            let meta = serde_json::json!({ "url": target }).to_string();
            crate::db::log_event(
                &event_id, &cid, &sid, &tenant_id,
                "click", &variant, &meta, Some("direct"),
            ).ok();
        }
    }

    Redirect::to(&target)
}

// ── Unsubscribe ───────────────────────────────────────────────────────────────

async fn handle_unsubscribe(Path(id): Path<String>) -> Html<String> {
    if id.starts_with("test-") {
        let email = decode_test_email(&id);
        tracing::info!("🧪 TEST unsubscribe for: {}", email);
        return Html(unsubscribe_page_test(&email));
    }

    let result = crate::db::with_db(|conn| {
        let row = conn.query_row(
            "SELECT to_email, subscriber_id, campaign_id, ab_variant, tenant_id
             FROM delivery_queue WHERE tracking_id = ?1",
            rusqlite::params![&id],
            |row| Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, String>(3)?,
                row.get::<_, String>(4)?,
            )),
        ).ok();
        Ok(row)
    }).unwrap_or(None);

    if let Some((email, sub_id, camp_id, variant, tenant_id)) = result {
        // Add to suppression list
        crate::db::with_db(|conn| {
            conn.execute(
                "INSERT OR REPLACE INTO suppressions (email, tenant_id, reason) VALUES (?1, ?2, 'unsubscribe')",
                rusqlite::params![&email, &tenant_id],
            )?;
            Ok(())
        }).ok();

        let event_id = format!("unsub-{}", id);
        crate::db::log_event(
            &event_id, &camp_id, &sub_id, &tenant_id,
            "unsubscribe", &variant, "{}", Some("direct"),
        ).ok();

        tracing::info!("✅ Unsubscribed: {} from campaign={}", email, camp_id);
        return Html(unsubscribe_page_success(&email));
    }

    Html(unsubscribe_page_invalid())
}

async fn handle_one_click_unsubscribe(Path(id): Path<String>) -> Response {
    if id.starts_with("test-") {
        let email = decode_test_email(&id);
        tracing::info!("🧪 TEST one-click unsubscribe for: {}", email);
        return Response::builder()
            .status(200)
            .header("Content-Type", "text/plain")
            .body(axum::body::Body::from("Unsubscribed"))
            .unwrap();
    }

    let result = crate::db::with_db(|conn| {
        let row = conn.query_row(
            "SELECT to_email, subscriber_id, campaign_id, ab_variant, tenant_id
             FROM delivery_queue WHERE tracking_id = ?1",
            rusqlite::params![&id],
            |row| Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, String>(3)?,
                row.get::<_, String>(4)?,
            )),
        ).ok();
        Ok(row)
    }).unwrap_or(None);

    if let Some((email, sub_id, camp_id, variant, tenant_id)) = result {
        crate::db::with_db(|conn| {
            conn.execute(
                "INSERT OR REPLACE INTO suppressions (email, tenant_id, reason) VALUES (?1, ?2, 'unsubscribe')",
                rusqlite::params![&email, &tenant_id],
            )?;
            Ok(())
        }).ok();

        let event_id = format!("unsub-oneclick-{}", id);
        crate::db::log_event(
            &event_id, &camp_id, &sub_id, &tenant_id,
            "unsubscribe", &variant, r#"{"method":"one-click"}"#, Some("direct"),
        ).ok();
    }

    // RFC 8058 always returns 200
    Response::builder()
        .status(200)
        .header("Content-Type", "text/plain")
        .body(axum::body::Body::from("Unsubscribed"))
        .unwrap()
}

// ── Helpers ───────────────────────────────────────────────────────────────────

fn decode_b64_url(encoded: &str) -> String {
    base64::engine::general_purpose::STANDARD
        .decode(encoded)
        .ok()
        .and_then(|b| String::from_utf8(b).ok())
        .unwrap_or_else(|| "https://boalix.com".into())
}

fn decode_test_email(id: &str) -> String {
    id.split('-')
        .nth(1)
        .and_then(|h| hex::decode(h).ok())
        .and_then(|bytes| String::from_utf8(bytes).ok())
        .unwrap_or_else(|| "test-recipient@example.com".to_string())
}

// ── HTML Pages ────────────────────────────────────────────────────────────────

fn unsubscribe_page_test(email: &str) -> String {
    format!(r#"<!DOCTYPE html>
<html lang="en"><head><meta charset="UTF-8"><meta name="viewport" content="width=device-width, initial-scale=1.0">
<title>Unsubscribed (Test Mode)</title>
<style>
  body {{ font-family: -apple-system, BlinkMacSystemFont, "Segoe UI", sans-serif; display:flex; align-items:center; justify-content:center; min-height:100vh; background:#f4f5f6; margin:0; }}
  .card {{ text-align:center; background:#fff; padding:48px; border-radius:24px; border:1px solid rgba(0,0,0,.08); box-shadow:0 20px 25px -5px rgba(0,0,0,.03); max-width:450px; width:100%; box-sizing:border-box; }}
  .icon {{ width:64px; height:64px; background:#f0fdf4; border:1px solid #dcfce7; border-radius:50%; display:flex; align-items:center; justify-content:center; margin:0 auto 24px; color:#16a34a; }}
  .badge {{ display:inline-block; background:#eff6ff; border:1px solid #bfdbfe; color:#1d4ed8; font-size:11px; font-weight:700; padding:3px 8px; border-radius:6px; text-transform:uppercase; letter-spacing:.05em; margin-bottom:16px; }}
  h1 {{ margin:0 0 12px; font-size:24px; font-weight:700; color:#09090b; }}
  p {{ color:#4b5563; margin:0; font-size:14px; line-height:1.6; }}
  .pill {{ display:inline-block; background:#f4f4f5; border:1px solid #e4e4e7; padding:6px 16px; border-radius:9999px; font-family:monospace; font-size:13px; color:#27272a; margin-top:16px; }}
  .notice {{ margin-top:20px; padding:12px; background:#f8fafc; border:1px solid #e2e8f0; border-radius:8px; font-size:12px; color:#64748b; }}
  .footer {{ margin-top:32px; font-size:12px; font-weight:600; color:#a1a1aa; letter-spacing:.05em; text-transform:uppercase; }}
</style></head><body>
  <div class="card">
    <div class="icon"><svg width="28" height="28" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.5" stroke-linecap="round" stroke-linejoin="round"><path d="M20 6L9 17l-5-5"/></svg></div>
    <div class="badge">Test Mode</div>
    <h1>Unsubscribed</h1>
    <p>This is a simulated unsubscribe flow. In a live campaign, this would remove:</p>
    <div class="pill">{}</div>
    <div class="notice"><strong>Verification successful:</strong> Tracking server is fully operational.</div>
    <div class="footer">Powered by Boalix</div>
  </div>
</body></html>"#, email)
}

fn unsubscribe_page_success(email: &str) -> String {
    format!(r#"<!DOCTYPE html>
<html lang="en"><head><meta charset="UTF-8"><meta name="viewport" content="width=device-width, initial-scale=1.0">
<title>Unsubscribed</title>
<style>
  body {{ font-family: -apple-system, BlinkMacSystemFont, "Segoe UI", sans-serif; display:flex; align-items:center; justify-content:center; min-height:100vh; background:#f4f5f6; margin:0; }}
  .card {{ text-align:center; background:#fff; padding:48px; border-radius:24px; border:1px solid rgba(0,0,0,.08); box-shadow:0 20px 25px -5px rgba(0,0,0,.03); max-width:420px; width:100%; box-sizing:border-box; }}
  .icon {{ width:64px; height:64px; background:#f0fdf4; border:1px solid #dcfce7; border-radius:50%; display:flex; align-items:center; justify-content:center; margin:0 auto 24px; color:#16a34a; }}
  h1 {{ margin:0 0 12px; font-size:24px; font-weight:700; color:#09090b; }}
  p {{ color:#4b5563; margin:0; font-size:14px; line-height:1.6; }}
  .pill {{ display:inline-block; background:#f4f4f5; border:1px solid #e4e4e7; padding:6px 16px; border-radius:9999px; font-family:monospace; font-size:13px; color:#27272a; margin-top:16px; }}
  .footer {{ margin-top:32px; font-size:12px; font-weight:600; color:#a1a1aa; letter-spacing:.05em; text-transform:uppercase; }}
</style></head><body>
  <div class="card">
    <div class="icon"><svg width="28" height="28" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.5" stroke-linecap="round" stroke-linejoin="round"><path d="M20 6L9 17l-5-5"/></svg></div>
    <h1>Unsubscribed</h1>
    <p>Your preferences have been updated. We've removed the following address from our mailing lists:</p>
    <div class="pill">{}</div>
    <div class="footer">Powered by Boalix</div>
  </div>
</body></html>"#, email)
}

fn unsubscribe_page_invalid() -> String {
    r#"<!DOCTYPE html>
<html lang="en"><head><meta charset="UTF-8"><title>Invalid Link</title>
<style>
  body { font-family: -apple-system, sans-serif; display:flex; align-items:center; justify-content:center; min-height:100vh; background:#f4f5f6; margin:0; }
  .card { text-align:center; background:#fff; padding:48px; border-radius:24px; max-width:420px; width:100%; }
  h1 { color:#09090b; } p { color:#4b5563; font-size:14px; }
</style></head><body>
  <div class="card">
    <h1>Invalid Link</h1>
    <p>This unsubscribe link is invalid, expired, or has already been processed.</p>
  </div>
</body></html>"#.to_string()
}
