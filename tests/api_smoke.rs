//! Smoke test for the router shape that doesn't need a database.
//!
//! Boots `api::router` against an `AppState` with a fresh `BlobStore`
//! and verifies the auth-gated endpoints reject anonymous traffic with
//! 401 while the public ones (healthz, GET /api/v1/pkg) reach their
//! handlers. The actual DB-touching paths are covered in tests
//! under a future `tests/api_db.rs` once we wire `testcontainers`.

use std::sync::Arc;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use jwc_registry::{api, config::Config, storage::BlobStore, AppState};
use tower::ServiceExt;

fn dummy_config() -> Config {
    Config {
        bind: "127.0.0.1:0".into(),
        database_url: "postgres://nowhere/none".into(),
        storage_path: std::path::PathBuf::from("."),
        max_upload_bytes: 1024 * 1024,
        google_client_id: "stub-client".into(),
        google_client_secret: "stub-secret".into(),
        google_redirect_uri: "http://localhost/cb".into(),
        jwt_secret: "test-secret-jwt".into(),
        jwt_ttl_secs: 60,
    }
}

fn router_for(_tmp: &std::path::Path) -> axum::Router {
    let cfg = dummy_config();
    let store = BlobStore::new(_tmp.to_path_buf()).unwrap();
    // For a pool-free smoke we still need *something* — build a pool
    // pointing at an unreachable address. Handlers that touch the DB
    // will fail with 500, but the auth + body-shape checks (which run
    // before DB access) still cover useful ground.
    let pool = deadpool_postgres::Pool::builder(deadpool_postgres::Manager::from_config(
        "host=127.0.0.1 port=1 user=none dbname=none"
            .parse::<tokio_postgres::Config>()
            .unwrap(),
        tokio_postgres::NoTls,
        deadpool_postgres::ManagerConfig {
            recycling_method: deadpool_postgres::RecyclingMethod::Fast,
        },
    ))
    .max_size(1)
    .build()
    .unwrap();
    let state = AppState {
        config: Arc::new(cfg),
        db: pool,
        storage: Arc::new(store),
    };
    api::router(state)
}

#[tokio::test]
async fn healthz_returns_ok() {
    let tmp = tempfile::tempdir().unwrap();
    let app = router_for(tmp.path());
    let resp = app
        .oneshot(
            Request::builder()
                .uri("/healthz")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
}

#[tokio::test]
async fn me_without_auth_is_unauthorized() {
    let tmp = tempfile::tempdir().unwrap();
    let app = router_for(tmp.path());
    let resp = app
        .oneshot(
            Request::builder()
                .uri("/api/v1/me")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn upload_without_auth_is_unauthorized() {
    let tmp = tempfile::tempdir().unwrap();
    let app = router_for(tmp.path());
    let resp = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/pkg/logger/0.1.0")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn me_with_garbage_bearer_is_unauthorized() {
    let tmp = tempfile::tempdir().unwrap();
    let app = router_for(tmp.path());
    let resp = app
        .oneshot(
            Request::builder()
                .uri("/api/v1/me")
                .header("authorization", "Bearer not-a-real-jwt")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn crates_alias_route_exists() {
    let tmp = tempfile::tempdir().unwrap();
    let app = router_for(tmp.path());
    let resp = app
        .oneshot(
            Request::builder()
                .uri("/api/v1/crates/qr-lite")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_ne!(resp.status(), StatusCode::NOT_FOUND);
}
#[tokio::test]
async fn google_login_redirects_to_google() {
    let tmp = tempfile::tempdir().unwrap();
    let app = router_for(tmp.path());
    let resp = app
        .oneshot(
            Request::builder()
                .uri("/api/v1/auth/google/login")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert!(
        resp.status().is_redirection(),
        "expected redirect, got {}",
        resp.status()
    );
    let location = resp
        .headers()
        .get("location")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");
    assert!(
        location.starts_with("https://accounts.google.com/"),
        "expected Google URL, got {location}"
    );
    assert!(location.contains("client_id=stub-client"));
    assert!(location.contains("response_type=code"));
}
