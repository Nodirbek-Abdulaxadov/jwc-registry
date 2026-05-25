//! Router composition + extractor wiring.

use axum::async_trait;
use axum::extract::{DefaultBodyLimit, FromRequestParts, State};
use axum::http::{request::Parts, StatusCode};
use axum::response::{IntoResponse, Json};
use axum::routing::{delete, get, post};
use axum::Router;
use tower_http::compression::CompressionLayer;
use tower_http::cors::{Any, CorsLayer};
use tower_http::services::ServeDir;
use tower_http::trace::TraceLayer;

use crate::auth::{self, AuthUser};
use crate::{api_keys, packages, AppState};

/// Build the full axum router. Wired separately from `main` so the
/// integration tests can mount it against an arbitrary `AppState`.
pub fn router(state: AppState) -> Router {
    let max_upload = state.config.max_upload_bytes;
    Router::new()
        .route("/healthz", get(healthz))
        // Auth
        .route("/api/v1/auth/google/login", get(auth::login))
        .route("/api/v1/auth/google/callback", get(auth::callback))
        .route("/api/v1/me", get(me))
        // API keys
        .route(
            "/api/v1/keys",
            get(api_keys::list_keys).post(api_keys::create_key),
        )
        .route("/api/v1/keys/:id", delete(api_keys::revoke_key))
        // Packages
        .route("/api/v1/pkg", get(packages::list_packages))
        .route("/api/v1/pkg/:name", get(packages::get_package))
        // Backward-compatible alias used by older jwc CLI releases.
        .route("/api/v1/crates/:name", get(packages::get_package))
        .route("/api/v1/pkg/:name/:version", post(packages::upload_version))
        .route(
            "/api/v1/pkg/:name/:version",
            delete(packages::delete_version),
        )
        .route(
            "/api/v1/pkg/:name/:version/download",
            get(packages::download_version),
        )
        // Backward-compatible alias used by older jwc CLI releases.
        .route(
            "/api/v1/crates/:name/:version/download",
            get(packages::download_version),
        )
        // Static frontend — served from `./static/` next to the binary in
        // production (`COPY static /app/static` in the Dockerfile). API
        // routes above match first; `fallback_service` only fires when no
        // route matched, so `/api/...` is never shadowed.
        .fallback_service(ServeDir::new("static"))
        .layer(DefaultBodyLimit::max(max_upload + 4096)) // +4kb for multipart overhead
        .layer(CompressionLayer::new())
        .layer(
            CorsLayer::new()
                .allow_origin(Any)
                .allow_methods(Any)
                .allow_headers(Any),
        )
        .layer(TraceLayer::new_for_http())
        .with_state(state)
}

/// Liveness probe — used by `kubectl`/docker healthcheck/CI smoke.
async fn healthz() -> impl IntoResponse {
    Json(serde_json::json!({ "status": "ok" }))
}

/// `GET /api/v1/me` — confirm the current bearer token resolves to a
/// user. Useful for client `jwc login` to verify the token before
/// writing it to the credentials file.
async fn me(_: State<AppState>, user: AuthUser) -> impl IntoResponse {
    Json(serde_json::json!({
        "id": user.id.to_string(),
        "email": user.email,
    }))
}

/// Axum extractor: read `Authorization: Bearer <jwt>`, verify, return
/// `AuthUser`. 401 on any failure (missing, malformed, or expired).
#[async_trait]
impl FromRequestParts<AppState> for AuthUser {
    type Rejection = (StatusCode, Json<serde_json::Value>);

    async fn from_request_parts(
        parts: &mut Parts,
        state: &AppState,
    ) -> Result<Self, Self::Rejection> {
        let header = parts
            .headers
            .get("authorization")
            .and_then(|h| h.to_str().ok())
            .ok_or_else(|| {
                (
                    StatusCode::UNAUTHORIZED,
                    Json(serde_json::json!({ "error": "missing Authorization header" })),
                )
            })?;
        let token = header.strip_prefix("Bearer ").ok_or_else(|| {
            (
                StatusCode::UNAUTHORIZED,
                Json(serde_json::json!({
                    "error": "Authorization must be 'Bearer <jwt-or-jwc_key>'"
                })),
            )
        })?;

        // Dispatch on the token shape: API keys carry a `jwc_` prefix
        // and are stored hashed in Postgres; everything else is treated
        // as a session JWT and verified locally.
        if api_keys::is_api_key(token) {
            match api_keys::resolve_key(state, token).await {
                Ok(Some(user)) => Ok(user),
                Ok(None) => Err((
                    StatusCode::UNAUTHORIZED,
                    Json(serde_json::json!({
                        "error": "API key invalid or revoked"
                    })),
                )),
                Err(e) => Err((
                    StatusCode::INTERNAL_SERVER_ERROR,
                    Json(serde_json::json!({ "error": e.to_string() })),
                )),
            }
        } else {
            auth::verify_token(&state.config.jwt_secret, token).map_err(|e| {
                (
                    StatusCode::UNAUTHORIZED,
                    Json(serde_json::json!({ "error": e.to_string() })),
                )
            })
        }
    }
}
