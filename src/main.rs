//! jwc-registry binary entrypoint — thin axum boot wrapper.

use std::sync::Arc;

use anyhow::{Context, Result};
use jwc_registry::{api, config::Config, db, storage::BlobStore, AppState};
use tracing_subscriber::EnvFilter;

#[tokio::main]
async fn main() -> Result<()> {
    init_tracing();

    let cfg = Config::from_env().context("load runtime config")?;
    tracing::info!(bind = %cfg.bind, "starting jwc-registry");

    let pool = db::build_pool(&cfg.database_url).await?;
    db::run_migrations(&pool).await?;

    let store = BlobStore::new(cfg.storage_path.clone())?;
    let state = AppState {
        config: Arc::new(cfg.clone()),
        db: pool,
        storage: Arc::new(store),
    };

    let app = api::router(state);
    let listener = tokio::net::TcpListener::bind(&cfg.bind)
        .await
        .with_context(|| format!("binding {}", cfg.bind))?;
    tracing::info!("listening on {}", cfg.bind);
    axum::serve(listener, app).await.context("axum serve")?;
    Ok(())
}

fn init_tracing() {
    let filter = EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| EnvFilter::new("info,jwc_registry=debug,tower_http=info"));
    tracing_subscriber::fmt().with_env_filter(filter).init();
}
