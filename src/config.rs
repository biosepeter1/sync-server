// config.rs — reads environment variables for runtime configuration

/// Get the configured tracking domain — the public URL this server is reachable at.
/// This is what gets embedded in emails. Example: "https://track.boalix.io"
/// This can be changed at any time (env var or DB) if a domain gets blacklisted.
pub fn tracking_domain() -> String {
    std::env::var("TRACKING_DOMAIN")
        .unwrap_or_else(|_| "http://localhost:8080".to_string())
}

/// Master API secret used to validate tenant API keys.
/// In production: set a long random value in your env vars.
pub fn api_secret() -> String {
    std::env::var("API_SECRET")
        .unwrap_or_else(|_| "change-me-in-production".to_string())
}

/// Path to the SQLite database file.
pub fn database_path() -> String {
    std::env::var("DATABASE_PATH")
        .unwrap_or_else(|_| "./boalix-sync.db".to_string())
}
