# ── Build Stage ───────────────────────────────────────────────────────────────
FROM rust:1.82-slim AS builder

WORKDIR /app

# Install OpenSSL dev libs needed by some crates
RUN apt-get update && apt-get install -y pkg-config libssl-dev && rm -rf /var/lib/apt/lists/*

# Cache dependencies separately
COPY Cargo.toml ./
RUN mkdir src && echo 'fn main() {}' > src/main.rs
RUN cargo build --release 2>/dev/null || true
RUN rm -f target/release/deps/boalix_sync_server*

# Build the real binary
COPY src ./src
RUN cargo build --release

# ── Runtime Stage ─────────────────────────────────────────────────────────────
FROM debian:bookworm-slim AS runtime

WORKDIR /app

# Install OpenSSL runtime libs
RUN apt-get update && apt-get install -y ca-certificates libssl3 && rm -rf /var/lib/apt/lists/*

# Copy the compiled binary from builder
COPY --from=builder /app/target/release/boalix-sync-server /app/boalix-sync-server

# Create data directory for SQLite
RUN mkdir -p /data

# Environment defaults (override in Render/Railway/Fly dashboard)
ENV PORT=8080
ENV DATABASE_PATH=/data/boalix-sync.db
ENV TRACKING_DOMAIN=http://localhost:8080
ENV API_SECRET=change-me-in-production
ENV RUST_LOG=boalix_sync_server=info,warn

EXPOSE 8080

CMD ["/app/boalix-sync-server"]
