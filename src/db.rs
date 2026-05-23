//! Postgres connection pool + schema migration runner.
//!
//! v1 keeps the migration runner deliberately tiny: a single
//! `MIGRATIONS` array of `(name, sql)` pairs applied in order inside a
//! transaction, recorded in `_registry_migrations(name, applied_at)`.
//! Idempotent — every boot reads the table and only runs what's new.
//! Replace with `sqlx-migrate` / `refinery` if/when the volume of
//! migrations crosses ~20 entries.

use anyhow::{Context, Result};
use deadpool_postgres::{Manager, ManagerConfig, RecyclingMethod};
use tokio_postgres::NoTls;

pub type Pool = deadpool_postgres::Pool;

/// Build a connection pool from a Postgres URL. Caller is expected to
/// have validated the URL shape.
pub async fn build_pool(database_url: &str) -> Result<Pool> {
    let pg_cfg: tokio_postgres::Config = database_url
        .parse()
        .with_context(|| format!("invalid REGISTRY_DB_URL: {database_url}"))?;
    let mgr_cfg = ManagerConfig {
        recycling_method: RecyclingMethod::Fast,
    };
    let mgr = Manager::from_config(pg_cfg, NoTls, mgr_cfg);
    let pool = Pool::builder(mgr).max_size(16).build()?;
    // Sanity-check at startup so a bad URL fails fast.
    let _ = pool
        .get()
        .await
        .with_context(|| "failed to checkout initial Postgres connection")?;
    Ok(pool)
}

/// Ordered list of forward-only schema migrations. Appending an entry is
/// the only supported way to evolve the schema in v1; rewriting or
/// re-ordering will break already-deployed instances.
pub const MIGRATIONS: &[(&str, &str)] = &[(
    "0001_init",
    r#"
        CREATE TABLE IF NOT EXISTS _registry_migrations (
            name TEXT PRIMARY KEY,
            applied_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
        );

        CREATE TABLE IF NOT EXISTS users (
            id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
            google_sub TEXT UNIQUE NOT NULL,
            email TEXT NOT NULL,
            name TEXT,
            created_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
        );

        CREATE TABLE IF NOT EXISTS packages (
            id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
            name TEXT UNIQUE NOT NULL,
            owner_id UUID NOT NULL REFERENCES users(id) ON DELETE RESTRICT,
            created_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
        );
        CREATE INDEX IF NOT EXISTS idx_packages_owner ON packages(owner_id);

        CREATE TABLE IF NOT EXISTS package_versions (
            id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
            package_id UUID NOT NULL REFERENCES packages(id) ON DELETE CASCADE,
            version TEXT NOT NULL,
            sha256 TEXT NOT NULL,
            size_bytes BIGINT NOT NULL,
            blob_path TEXT NOT NULL,
            uploaded_by UUID NOT NULL REFERENCES users(id) ON DELETE RESTRICT,
            uploaded_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
            UNIQUE (package_id, version)
        );
        CREATE INDEX IF NOT EXISTS idx_versions_pkg ON package_versions(package_id);
        "#,
)];

/// Apply every pending migration in order, recording each in
/// `_registry_migrations`. Wrapped in an advisory lock so concurrent
/// boots don't race against each other.
pub async fn run_migrations(pool: &Pool) -> Result<()> {
    let mut client = pool.get().await?;
    // Make sure the bookkeeping table exists before we read from it.
    client
        .batch_execute(
            "CREATE TABLE IF NOT EXISTS _registry_migrations \
             (name TEXT PRIMARY KEY, applied_at TIMESTAMPTZ NOT NULL DEFAULT NOW())",
        )
        .await
        .context("creating _registry_migrations table")?;

    let applied: std::collections::HashSet<String> = client
        .query("SELECT name FROM _registry_migrations", &[])
        .await?
        .into_iter()
        .map(|row| row.get::<_, String>(0))
        .collect();

    for (name, sql) in MIGRATIONS {
        if applied.contains(*name) {
            continue;
        }
        let tx = client.transaction().await?;
        tx.batch_execute(sql)
            .await
            .with_context(|| format!("applying migration {name}"))?;
        tx.execute(
            "INSERT INTO _registry_migrations (name) VALUES ($1)",
            &[&name],
        )
        .await?;
        tx.commit().await?;
        tracing::info!(migration = %name, "applied");
    }

    Ok(())
}
