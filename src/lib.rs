//! JWC package registry — library crate.
//!
//! The binary in `src/main.rs` is a thin axum boot wrapper. Every
//! handler, schema, and helper lives here so integration tests in
//! `tests/` can exercise the same surface.
//!
//! Layout:
//! - `config`   — env-driven runtime config (DB url, OAuth secrets, ...).
//! - `auth`     — Google OAuth login + JWT session issuance.
//! - `db`       — deadpool-postgres pool + migration helpers.
//! - `storage`  — sha256-keyed blob store on the local filesystem.
//! - `packages` — package / version / download endpoints.
//! - `api`      — axum Router composition + middleware.

pub mod api;
pub mod auth;
pub mod config;
pub mod db;
pub mod packages;
pub mod storage;

/// Shared application state passed to every axum handler.
#[derive(Clone)]
pub struct AppState {
    pub config: std::sync::Arc<config::Config>,
    pub db: db::Pool,
    pub storage: std::sync::Arc<storage::BlobStore>,
}
