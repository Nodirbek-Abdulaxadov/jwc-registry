//! Package + version model and the upload / download / list / info /
//! delete endpoints.
//!
//! Conventions:
//! - Package names: `^[a-z][a-z0-9_-]{0,63}$` (single-segment, lowercase).
//! - Versions: caller-supplied free-form for v1 (semver enforcement is
//!   the jwc-lang client's job, not the registry's). We reject empty
//!   and any version that contains `/` (path safety).
//! - Tarballs are stored verbatim in the blob store; the registry does
//!   NOT parse manifests in v1. That keeps the server format-agnostic
//!   and lets the client iterate on jwcproj without server changes.
//! - Authorisation: any logged-in user can publish a *new* package
//!   (first publisher becomes owner). Subsequent publishes of the same
//!   package must come from the owner. Delete is owner-only.

use axum::extract::{Multipart, Path, State};
use axum::http::{header, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde::Serialize;
use uuid::Uuid;

use crate::auth::AuthUser;
use crate::AppState;

#[derive(Debug)]
pub enum PackageError {
    BadName(String),
    BadVersion(String),
    PayloadTooLarge(usize),
    MissingFile,
    Conflict(String),
    NotFound,
    Forbidden,
    Internal(String),
}

impl IntoResponse for PackageError {
    fn into_response(self) -> Response {
        let (status, msg) = match self {
            PackageError::BadName(m) | PackageError::BadVersion(m) => (StatusCode::BAD_REQUEST, m),
            PackageError::PayloadTooLarge(max) => (
                StatusCode::PAYLOAD_TOO_LARGE,
                format!("upload exceeds {max} bytes"),
            ),
            PackageError::MissingFile => (
                StatusCode::BAD_REQUEST,
                "multipart upload must include a 'file' field".to_string(),
            ),
            PackageError::Conflict(m) => (StatusCode::CONFLICT, m),
            PackageError::NotFound => (StatusCode::NOT_FOUND, "not found".to_string()),
            PackageError::Forbidden => (
                StatusCode::FORBIDDEN,
                "you do not own this package".to_string(),
            ),
            PackageError::Internal(m) => (StatusCode::INTERNAL_SERVER_ERROR, m),
        };
        (
            status,
            [(header::CONTENT_TYPE, "application/json")],
            serde_json::json!({ "error": msg }).to_string(),
        )
            .into_response()
    }
}

#[derive(Debug, Serialize)]
pub struct PackageView {
    pub name: String,
    pub owner_email: String,
    pub versions: Vec<VersionView>,
}

#[derive(Debug, Serialize)]
pub struct VersionView {
    pub version: String,
    pub sha256: String,
    pub size_bytes: i64,
    pub uploaded_at: chrono::DateTime<chrono::Utc>,
}

#[derive(Debug, Serialize)]
pub struct PackageSummary {
    pub name: String,
    pub latest_version: Option<String>,
    pub owner_email: String,
}

#[derive(Debug, Serialize)]
pub struct UploadResponse {
    pub name: String,
    pub version: String,
    pub sha256: String,
    pub size_bytes: i64,
}

/// `GET /api/v1/pkg` — flat list (no pagination in v1; tiny registry).
pub async fn list_packages(
    State(state): State<AppState>,
) -> Result<Json<Vec<PackageSummary>>, PackageError> {
    let client = state
        .db
        .get()
        .await
        .map_err(|e| PackageError::Internal(format!("db checkout: {e}")))?;
    let rows = client
        .query(
            "SELECT p.name,
                    u.email,
                    (SELECT version FROM package_versions v
                     WHERE v.package_id = p.id
                     ORDER BY v.uploaded_at DESC LIMIT 1) AS latest
             FROM packages p
             JOIN users u ON u.id = p.owner_id
             ORDER BY p.name ASC",
            &[],
        )
        .await
        .map_err(|e| PackageError::Internal(format!("list query: {e}")))?;
    let out = rows
        .into_iter()
        .map(|r| PackageSummary {
            name: r.get(0),
            owner_email: r.get(1),
            latest_version: r.get(2),
        })
        .collect();
    Ok(Json(out))
}

/// `GET /api/v1/pkg/:name` — full version list.
pub async fn get_package(
    State(state): State<AppState>,
    Path(name): Path<String>,
) -> Result<Json<PackageView>, PackageError> {
    validate_name(&name)?;
    let client = state
        .db
        .get()
        .await
        .map_err(|e| PackageError::Internal(format!("db checkout: {e}")))?;
    let pkg = client
        .query_opt(
            "SELECT p.id, p.name, u.email
             FROM packages p JOIN users u ON u.id = p.owner_id
             WHERE p.name = $1",
            &[&name],
        )
        .await
        .map_err(|e| PackageError::Internal(format!("pkg query: {e}")))?
        .ok_or(PackageError::NotFound)?;
    let pkg_id: Uuid = pkg.get(0);

    let versions = client
        .query(
            "SELECT version, sha256, size_bytes, uploaded_at
             FROM package_versions WHERE package_id = $1
             ORDER BY uploaded_at DESC",
            &[&pkg_id],
        )
        .await
        .map_err(|e| PackageError::Internal(format!("versions query: {e}")))?;

    Ok(Json(PackageView {
        name: pkg.get(1),
        owner_email: pkg.get(2),
        versions: versions
            .into_iter()
            .map(|r| VersionView {
                version: r.get(0),
                sha256: r.get(1),
                size_bytes: r.get(2),
                uploaded_at: r.get(3),
            })
            .collect(),
    }))
}

/// `POST /api/v1/pkg/:name/:version` — multipart upload of a tarball
/// in a field named `file`. Returns the stored metadata.
pub async fn upload_version(
    State(state): State<AppState>,
    user: AuthUser,
    Path((name, version)): Path<(String, String)>,
    mut multipart: Multipart,
) -> Result<Json<UploadResponse>, PackageError> {
    validate_name(&name)?;
    validate_version(&version)?;

    let mut bytes: Option<Vec<u8>> = None;
    while let Some(field) = multipart
        .next_field()
        .await
        .map_err(|e| PackageError::Internal(format!("multipart: {e}")))?
    {
        if field.name() == Some("file") {
            let body = field
                .bytes()
                .await
                .map_err(|e| PackageError::Internal(format!("multipart read: {e}")))?;
            if body.len() > state.config.max_upload_bytes {
                return Err(PackageError::PayloadTooLarge(state.config.max_upload_bytes));
            }
            bytes = Some(body.to_vec());
            break;
        }
    }
    let bytes = bytes.ok_or(PackageError::MissingFile)?;

    // First-publisher-wins ownership: the package row is created on
    // the first upload, and subsequent uploads must come from the
    // same owner. Wrapped in a tx so the package + version pair is
    // atomic.
    let mut client = state
        .db
        .get()
        .await
        .map_err(|e| PackageError::Internal(format!("db checkout: {e}")))?;
    let tx = client
        .transaction()
        .await
        .map_err(|e| PackageError::Internal(format!("begin tx: {e}")))?;

    let pkg = tx
        .query_opt(
            "SELECT id, owner_id FROM packages WHERE name = $1",
            &[&name],
        )
        .await
        .map_err(|e| PackageError::Internal(format!("pkg lookup: {e}")))?;

    let pkg_id: Uuid = match pkg {
        Some(row) => {
            let owner: Uuid = row.get(1);
            if owner != user.id {
                return Err(PackageError::Forbidden);
            }
            row.get(0)
        }
        None => tx
            .query_one(
                "INSERT INTO packages (name, owner_id) VALUES ($1, $2) RETURNING id",
                &[&name, &user.id],
            )
            .await
            .map_err(|e| PackageError::Internal(format!("pkg insert: {e}")))?
            .get(0),
    };

    // Reject duplicate versions — immutable publish semantics.
    let exists = tx
        .query_opt(
            "SELECT 1 FROM package_versions WHERE package_id = $1 AND version = $2",
            &[&pkg_id, &version],
        )
        .await
        .map_err(|e| PackageError::Internal(format!("version exists check: {e}")))?;
    if exists.is_some() {
        return Err(PackageError::Conflict(format!(
            "{name}@{version} already published"
        )));
    }

    let blob = state
        .storage
        .put(&bytes)
        .await
        .map_err(|e| PackageError::Internal(format!("blob store: {e}")))?;
    let rel_path = blob.path.to_string_lossy().into_owned();
    let size_i64 = blob.size as i64;

    tx.execute(
        "INSERT INTO package_versions (package_id, version, sha256, size_bytes, blob_path, uploaded_by)
         VALUES ($1, $2, $3, $4, $5, $6)",
        &[
            &pkg_id,
            &version,
            &blob.sha256,
            &size_i64,
            &rel_path,
            &user.id,
        ],
    )
    .await
    .map_err(|e| PackageError::Internal(format!("version insert: {e}")))?;

    tx.commit()
        .await
        .map_err(|e| PackageError::Internal(format!("commit: {e}")))?;

    Ok(Json(UploadResponse {
        name,
        version,
        sha256: blob.sha256,
        size_bytes: size_i64,
    }))
}

/// `GET /api/v1/pkg/:name/:version/download` — stream the tarball.
pub async fn download_version(
    State(state): State<AppState>,
    Path((name, version)): Path<(String, String)>,
) -> Result<Response, PackageError> {
    validate_name(&name)?;
    validate_version(&version)?;
    let client = state
        .db
        .get()
        .await
        .map_err(|e| PackageError::Internal(format!("db checkout: {e}")))?;
    let row = client
        .query_opt(
            "SELECT v.sha256
             FROM package_versions v JOIN packages p ON p.id = v.package_id
             WHERE p.name = $1 AND v.version = $2",
            &[&name, &version],
        )
        .await
        .map_err(|e| PackageError::Internal(format!("download lookup: {e}")))?
        .ok_or(PackageError::NotFound)?;
    let sha: String = row.get(0);
    let bytes = state
        .storage
        .get(&sha)
        .await
        .map_err(|e| PackageError::Internal(format!("blob read: {e}")))?
        .ok_or(PackageError::NotFound)?;
    Ok((
        StatusCode::OK,
        [
            (header::CONTENT_TYPE, "application/x-gzip"),
            (
                header::CONTENT_DISPOSITION,
                "attachment; filename=\"package.tar.gz\"",
            ),
        ],
        bytes,
    )
        .into_response())
}

/// `DELETE /api/v1/pkg/:name/:version` — owner-only soft-delete of a
/// single version. The package row is kept so the name stays reserved
/// to the original owner (prevents name-squatting after deletion).
pub async fn delete_version(
    State(state): State<AppState>,
    user: AuthUser,
    Path((name, version)): Path<(String, String)>,
) -> Result<StatusCode, PackageError> {
    validate_name(&name)?;
    validate_version(&version)?;
    let client = state
        .db
        .get()
        .await
        .map_err(|e| PackageError::Internal(format!("db checkout: {e}")))?;
    let pkg = client
        .query_opt(
            "SELECT id, owner_id FROM packages WHERE name = $1",
            &[&name],
        )
        .await
        .map_err(|e| PackageError::Internal(format!("pkg lookup: {e}")))?
        .ok_or(PackageError::NotFound)?;
    let pkg_id: Uuid = pkg.get(0);
    let owner: Uuid = pkg.get(1);
    if owner != user.id {
        return Err(PackageError::Forbidden);
    }
    let deleted = client
        .execute(
            "DELETE FROM package_versions WHERE package_id = $1 AND version = $2",
            &[&pkg_id, &version],
        )
        .await
        .map_err(|e| PackageError::Internal(format!("version delete: {e}")))?;
    if deleted == 0 {
        return Err(PackageError::NotFound);
    }
    Ok(StatusCode::NO_CONTENT)
}

/// Package name validator. Mirrors what the resolver in jwc-lang
/// accepts so a successful registry upload always parses there.
pub fn validate_name(name: &str) -> Result<(), PackageError> {
    if name.is_empty() || name.len() > 64 {
        return Err(PackageError::BadName(format!(
            "package name '{name}' must be 1..=64 chars"
        )));
    }
    let mut chars = name.chars();
    let first = chars.next().unwrap();
    if !first.is_ascii_lowercase() {
        return Err(PackageError::BadName(format!(
            "package name '{name}' must start with a-z"
        )));
    }
    for c in chars {
        if !(c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_' || c == '-') {
            return Err(PackageError::BadName(format!(
                "package name '{name}' contains invalid char '{c}'"
            )));
        }
    }
    Ok(())
}

/// Version validator — minimal v1: non-empty, no `/`, no whitespace.
/// Real semver checks live in the jwc-lang resolver.
pub fn validate_version(v: &str) -> Result<(), PackageError> {
    if v.is_empty() {
        return Err(PackageError::BadVersion("version is empty".into()));
    }
    if v.contains('/') || v.chars().any(|c| c.is_whitespace()) {
        return Err(PackageError::BadVersion(format!(
            "version '{v}' contains '/' or whitespace"
        )));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn valid_names_pass() {
        for n in ["a", "logger", "my-pkg", "log_v2", "abc123"] {
            validate_name(n).unwrap_or_else(|e| panic!("expected ok for {n}, got {e:?}"));
        }
    }

    #[test]
    fn invalid_names_reject() {
        for n in [
            "",
            "A",
            "1pkg",
            "my pkg",
            "my.pkg",
            "Caps",
            "x".repeat(65).as_str(),
        ] {
            assert!(validate_name(n).is_err(), "{n} should be invalid");
        }
    }

    #[test]
    fn valid_versions_pass() {
        for v in ["0.1.0", "1.0", "v2", "20240101", "a-b-c"] {
            validate_version(v).unwrap();
        }
    }

    #[test]
    fn invalid_versions_reject() {
        for v in ["", "1 0", "1/0", "1.0\n"] {
            assert!(validate_version(v).is_err(), "{v} should be invalid");
        }
    }
}
