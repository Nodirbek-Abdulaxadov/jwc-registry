//! API key management: create / list / revoke + Bearer verification path.
//!
//! Keys are stored as `sha256(plaintext)` hex — the plaintext is shown
//! exactly once on creation and never persisted. Format:
//! `jwc_<48-hex-char-random>` (28 bytes of entropy after the prefix).
//!
//! On the wire, callers send either a session JWT (Google login flow,
//! short-lived) or an API key in the same `Authorization: Bearer ...`
//! header. The `auth_with_bearer` extractor branches on the `jwc_` prefix:
//! - starts with `jwc_` → look up `api_keys.key_hash`
//! - anything else → verify as JWT
//!
//! The unified path means `jwc publish` / `jwc add` works with either
//! credential without the caller having to declare which kind they hold.

use axum::extract::{Path, State};
use axum::http::{header, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::Json;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use uuid::Uuid;

use crate::auth::AuthUser;
use crate::AppState;

#[derive(Debug)]
pub enum KeyError {
    BadInput(String),
    NotFound,
    Internal(String),
}

impl IntoResponse for KeyError {
    fn into_response(self) -> Response {
        let (status, msg) = match self {
            KeyError::BadInput(m) => (StatusCode::BAD_REQUEST, m),
            KeyError::NotFound => (StatusCode::NOT_FOUND, "not found".into()),
            KeyError::Internal(m) => (StatusCode::INTERNAL_SERVER_ERROR, m),
        };
        (
            status,
            [(header::CONTENT_TYPE, "application/json")],
            serde_json::json!({ "error": msg }).to_string(),
        )
            .into_response()
    }
}

#[derive(Debug, Deserialize)]
pub struct CreateKeyRequest {
    pub name: String,
}

#[derive(Debug, Serialize)]
pub struct CreateKeyResponse {
    pub id: Uuid,
    pub name: String,
    pub prefix: String,
    /// Returned ONCE on creation. The user copies it; we never store it.
    pub plaintext: String,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Serialize)]
pub struct KeyView {
    pub id: Uuid,
    pub name: String,
    pub prefix: String,
    pub created_at: DateTime<Utc>,
    pub last_used_at: Option<DateTime<Utc>>,
}

/// `POST /api/v1/keys` — body `{name}`, response includes plaintext.
pub async fn create_key(
    State(state): State<AppState>,
    user: AuthUser,
    Json(req): Json<CreateKeyRequest>,
) -> Result<Json<CreateKeyResponse>, KeyError> {
    let name = req.name.trim();
    if name.is_empty() || name.len() > 64 {
        return Err(KeyError::BadInput("key name must be 1..=64 chars".into()));
    }
    let plaintext = generate_plaintext();
    let prefix: String = plaintext.chars().take(12).collect();
    let key_hash = sha256_hex(plaintext.as_bytes());

    let client = state
        .db
        .get()
        .await
        .map_err(|e| KeyError::Internal(format!("db checkout: {e}")))?;
    let row = client
        .query_one(
            "INSERT INTO api_keys (user_id, name, key_hash, prefix)
             VALUES ($1, $2, $3, $4)
             RETURNING id, created_at",
            &[&user.id, &name, &key_hash, &prefix],
        )
        .await
        .map_err(|e| KeyError::Internal(format!("key insert: {e}")))?;

    Ok(Json(CreateKeyResponse {
        id: row.get(0),
        name: name.to_string(),
        prefix,
        plaintext,
        created_at: row.get(1),
    }))
}

/// `GET /api/v1/keys` — current user's non-revoked keys.
pub async fn list_keys(
    State(state): State<AppState>,
    user: AuthUser,
) -> Result<Json<Vec<KeyView>>, KeyError> {
    let client = state
        .db
        .get()
        .await
        .map_err(|e| KeyError::Internal(format!("db checkout: {e}")))?;
    let rows = client
        .query(
            "SELECT id, name, prefix, created_at, last_used_at
             FROM api_keys
             WHERE user_id = $1 AND revoked_at IS NULL
             ORDER BY created_at DESC",
            &[&user.id],
        )
        .await
        .map_err(|e| KeyError::Internal(format!("list keys: {e}")))?;
    Ok(Json(
        rows.into_iter()
            .map(|r| KeyView {
                id: r.get(0),
                name: r.get(1),
                prefix: r.get(2),
                created_at: r.get(3),
                last_used_at: r.get(4),
            })
            .collect(),
    ))
}

/// `DELETE /api/v1/keys/:id` — soft-revoke (sets `revoked_at`). Idempotent.
pub async fn revoke_key(
    State(state): State<AppState>,
    user: AuthUser,
    Path(id): Path<Uuid>,
) -> Result<StatusCode, KeyError> {
    let client = state
        .db
        .get()
        .await
        .map_err(|e| KeyError::Internal(format!("db checkout: {e}")))?;
    let n = client
        .execute(
            "UPDATE api_keys SET revoked_at = NOW()
             WHERE id = $1 AND user_id = $2 AND revoked_at IS NULL",
            &[&id, &user.id],
        )
        .await
        .map_err(|e| KeyError::Internal(format!("revoke: {e}")))?;
    if n == 0 {
        return Err(KeyError::NotFound);
    }
    Ok(StatusCode::NO_CONTENT)
}

/// Look up a plaintext API key, return the owning user. Bumps
/// `last_used_at` as a side effect. Returns `Ok(None)` for unknown
/// or revoked keys.
pub async fn resolve_key(state: &AppState, plaintext: &str) -> anyhow::Result<Option<AuthUser>> {
    let hash = sha256_hex(plaintext.as_bytes());
    let client = state.db.get().await?;
    let row = client
        .query_opt(
            "UPDATE api_keys SET last_used_at = NOW()
             WHERE key_hash = $1 AND revoked_at IS NULL
             RETURNING user_id",
            &[&hash],
        )
        .await?;
    let Some(row) = row else { return Ok(None) };
    let user_id: Uuid = row.get(0);
    let urow = client
        .query_one("SELECT email FROM users WHERE id = $1", &[&user_id])
        .await?;
    Ok(Some(AuthUser {
        id: user_id,
        email: urow.get(0),
    }))
}

pub fn is_api_key(token: &str) -> bool {
    token.starts_with("jwc_")
}

fn generate_plaintext() -> String {
    use std::time::SystemTime;
    // 28 random bytes via UUID v4 pair → 56 hex chars; prefix `jwc_`.
    let a = Uuid::new_v4().as_bytes().to_vec();
    let b = Uuid::new_v4().as_bytes().to_vec();
    // Mix in process time so two parallel calls inside the same μs are
    // still distinct even if /dev/urandom happens to seed identically.
    let t = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0)
        .to_le_bytes();
    let mixed: Vec<u8> = a.iter().chain(b.iter()).chain(t.iter()).copied().collect();
    let mut h = Sha256::new();
    h.update(&mixed);
    let digest = h.finalize();
    format!("jwc_{}", hex::encode(&digest[..28]))
}

fn sha256_hex(bytes: &[u8]) -> String {
    let mut h = Sha256::new();
    h.update(bytes);
    hex::encode(h.finalize())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn generated_keys_have_expected_shape() {
        let k = generate_plaintext();
        assert!(k.starts_with("jwc_"));
        assert_eq!(k.len(), 4 + 56); // prefix + 28 bytes hex
    }

    #[test]
    fn generated_keys_are_unique() {
        let a = generate_plaintext();
        let b = generate_plaintext();
        assert_ne!(a, b);
    }

    #[test]
    fn is_api_key_recognises_prefix() {
        assert!(is_api_key("jwc_abc"));
        assert!(!is_api_key("eyJ.jwt.token"));
        assert!(!is_api_key(""));
    }
}
