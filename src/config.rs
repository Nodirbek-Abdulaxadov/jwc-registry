//! Env-driven runtime configuration.
//!
//! Read once at startup; everything downstream consumes `&Config`.
//! Missing required variables fail-fast at boot (not at first request)
//! so misconfigured deploys never reach a half-up state.

use anyhow::{anyhow, Context, Result};

#[derive(Debug, Clone)]
pub struct Config {
    /// `host:port` the axum server binds to.
    pub bind: String,
    /// Postgres connection URL (`postgres://user:pwd@host/db`).
    pub database_url: String,
    /// Filesystem root where sha256-keyed blobs are stored.
    pub storage_path: std::path::PathBuf,
    /// Bytes — uploads above this size are rejected with 413.
    pub max_upload_bytes: usize,
    /// Google OAuth client id (from console.cloud.google.com).
    pub google_client_id: String,
    /// Google OAuth client secret.
    pub google_client_secret: String,
    /// Full callback URL registered with Google (e.g.
    /// `https://registry-jwc.1kb.uz/api/v1/auth/google/callback`).
    pub google_redirect_uri: String,
    /// HMAC secret used to sign session JWTs.
    pub jwt_secret: String,
    /// JWT lifetime in seconds.
    pub jwt_ttl_secs: u64,
}

impl Config {
    pub fn from_env() -> Result<Self> {
        Ok(Self {
            bind: env_or("REGISTRY_BIND", "0.0.0.0:8080"),
            database_url: env_required("REGISTRY_DB_URL")?,
            storage_path: std::path::PathBuf::from(env_or("REGISTRY_STORAGE_PATH", "./storage")),
            max_upload_bytes: env_or("REGISTRY_MAX_UPLOAD_BYTES", "52428800")
                .parse()
                .context("REGISTRY_MAX_UPLOAD_BYTES must be an integer")?,
            google_client_id: env_required("REGISTRY_GOOGLE_CLIENT_ID")?,
            google_client_secret: env_required("REGISTRY_GOOGLE_CLIENT_SECRET")?,
            google_redirect_uri: env_required("REGISTRY_GOOGLE_REDIRECT_URI")?,
            jwt_secret: env_required("REGISTRY_JWT_SECRET")?,
            jwt_ttl_secs: env_or("REGISTRY_JWT_TTL_SECS", "604800")
                .parse()
                .context("REGISTRY_JWT_TTL_SECS must be an integer")?,
        })
    }
}

fn env_required(key: &str) -> Result<String> {
    std::env::var(key).map_err(|_| anyhow!("missing required env var: {key}"))
}

fn env_or(key: &str, default: &str) -> String {
    std::env::var(key).unwrap_or_else(|_| default.to_string())
}
