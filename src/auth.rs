//! Google OAuth 2.0 authorization-code flow + JWT session issuance.
//!
//! Flow:
//! 1. `GET /api/v1/auth/google/login` — server builds the Google
//!    consent URL and returns a 302 redirect to it.
//! 2. User accepts on Google's UI; Google redirects to our
//!    `google_redirect_uri` with `?code=...&state=...`.
//! 3. `GET /api/v1/auth/google/callback` — server swaps the code for
//!    an access token, fetches the user's profile (sub/email/name),
//!    upserts a `users` row, and returns a signed JWT.
//! 4. Subsequent API calls attach `Authorization: Bearer <jwt>`.
//!    `current_user` extractor verifies the signature, loads the user
//!    from Postgres, and hands a typed `AuthUser` to handlers.

use anyhow::{anyhow, Context, Result};
use axum::extract::{Query, State};
use axum::http::{header, StatusCode};
use axum::response::{IntoResponse, Redirect, Response};
use axum::Json;
use chrono::{Duration, Utc};
use jsonwebtoken::{decode, encode, DecodingKey, EncodingKey, Header, Validation};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::AppState;

/// Authenticated user — populated by the `auth_extractor` middleware
/// once a request passes JWT verification.
#[derive(Debug, Clone)]
pub struct AuthUser {
    pub id: Uuid,
    pub email: String,
}

/// JWT claims body — kept small on purpose. `sub` is our internal user
/// id (Uuid string); `email` is denormalised so we don't have to hit
/// Postgres for trivial display.
#[derive(Debug, Serialize, Deserialize)]
pub struct Claims {
    pub sub: String,
    pub email: String,
    pub exp: i64,
}

/// `GET /api/v1/auth/google/login` — kick off the OAuth dance.
pub async fn login(State(state): State<AppState>) -> Response {
    let url = format!(
        "https://accounts.google.com/o/oauth2/v2/auth\
         ?client_id={cid}\
         &redirect_uri={ruri}\
         &response_type=code\
         &scope={scope}\
         &access_type=online\
         &prompt=select_account",
        cid = urlencode(&state.config.google_client_id),
        ruri = urlencode(&state.config.google_redirect_uri),
        scope = urlencode("openid email profile"),
    );
    Redirect::to(&url).into_response()
}

#[derive(Debug, Deserialize)]
pub struct CallbackParams {
    pub code: String,
    /// `?format=json` — CLI flow. Omitted = browser flow (redirect to /).
    #[serde(default)]
    pub format: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct LoginResponse {
    pub token: String,
    pub user: PublicUser,
}

#[derive(Debug, Serialize)]
pub struct PublicUser {
    pub id: Uuid,
    pub email: String,
    pub name: Option<String>,
}

/// `GET /api/v1/auth/google/callback?code=...` — exchange + upsert + issue JWT.
///
/// Default (browser) flow: redirects to `/?token=<jwt>` so the SPA at the
/// root stashes it in localStorage. Pass `?format=json` to receive the
/// `{token, user}` body verbatim (used by `jwc login` from the CLI).
pub async fn callback(
    State(state): State<AppState>,
    Query(params): Query<CallbackParams>,
) -> Result<Response, AuthError> {
    let token = exchange_code_for_token(&state, &params.code).await?;
    let profile = fetch_google_profile(&token).await?;
    let user = upsert_user(&state, &profile).await?;
    let jwt = issue_jwt(&state, &user)?;
    let body = LoginResponse {
        token: jwt.clone(),
        user: PublicUser {
            id: user.id,
            email: user.email,
            name: user.name,
        },
    };
    if params.format.as_deref() == Some("json") {
        return Ok(Json(body).into_response());
    }
    // Browser flow: hand the token to the SPA via `?token=...`.
    Ok(Redirect::to(&format!("/?token={}", urlencode(&jwt))).into_response())
}

/// Internal — POST to Google's token endpoint to swap auth code for an
/// access token. Errors map straight onto `AuthError::Upstream`.
async fn exchange_code_for_token(state: &AppState, code: &str) -> Result<String, AuthError> {
    let client = reqwest::Client::new();
    let resp = client
        .post("https://oauth2.googleapis.com/token")
        .form(&[
            ("code", code),
            ("client_id", &state.config.google_client_id),
            ("client_secret", &state.config.google_client_secret),
            ("redirect_uri", &state.config.google_redirect_uri),
            ("grant_type", "authorization_code"),
        ])
        .send()
        .await
        .map_err(|e| AuthError::Upstream(format!("token exchange: {e}")))?;

    if !resp.status().is_success() {
        let body = resp.text().await.unwrap_or_default();
        return Err(AuthError::Upstream(format!(
            "Google token endpoint returned non-2xx: {body}"
        )));
    }

    #[derive(Deserialize)]
    struct TokenResp {
        access_token: String,
    }
    let parsed: TokenResp = resp
        .json()
        .await
        .map_err(|e| AuthError::Upstream(format!("token parse: {e}")))?;
    Ok(parsed.access_token)
}

#[derive(Debug, Deserialize)]
pub struct GoogleProfile {
    pub sub: String,
    pub email: String,
    pub name: Option<String>,
}

/// Internal — fetch the user's profile from Google's userinfo endpoint.
async fn fetch_google_profile(access_token: &str) -> Result<GoogleProfile, AuthError> {
    let resp = reqwest::Client::new()
        .get("https://openidconnect.googleapis.com/v1/userinfo")
        .bearer_auth(access_token)
        .send()
        .await
        .map_err(|e| AuthError::Upstream(format!("userinfo: {e}")))?;
    if !resp.status().is_success() {
        let body = resp.text().await.unwrap_or_default();
        return Err(AuthError::Upstream(format!(
            "Google userinfo returned non-2xx: {body}"
        )));
    }
    resp.json::<GoogleProfile>()
        .await
        .map_err(|e| AuthError::Upstream(format!("userinfo parse: {e}")))
}

#[derive(Debug, Clone)]
pub struct UserRow {
    pub id: Uuid,
    pub email: String,
    pub name: Option<String>,
}

/// Insert (or update name/email of) the user keyed by `google_sub`.
async fn upsert_user(state: &AppState, p: &GoogleProfile) -> Result<UserRow, AuthError> {
    let client = state
        .db
        .get()
        .await
        .map_err(|e| AuthError::Internal(format!("db checkout: {e}")))?;
    let row = client
        .query_one(
            "INSERT INTO users (google_sub, email, name) VALUES ($1, $2, $3)
             ON CONFLICT (google_sub) DO UPDATE SET email = EXCLUDED.email, name = EXCLUDED.name
             RETURNING id, email, name",
            &[&p.sub, &p.email, &p.name],
        )
        .await
        .map_err(|e| AuthError::Internal(format!("user upsert: {e}")))?;
    Ok(UserRow {
        id: row.get(0),
        email: row.get(1),
        name: row.get(2),
    })
}

fn issue_jwt(state: &AppState, user: &UserRow) -> Result<String, AuthError> {
    let exp = (Utc::now() + Duration::seconds(state.config.jwt_ttl_secs as i64)).timestamp();
    let claims = Claims {
        sub: user.id.to_string(),
        email: user.email.clone(),
        exp,
    };
    encode(
        &Header::default(),
        &claims,
        &EncodingKey::from_secret(state.config.jwt_secret.as_bytes()),
    )
    .map_err(|e| AuthError::Internal(format!("jwt encode: {e}")))
}

/// Verify a `Bearer <jwt>` header and return the embedded user id.
/// Used by handlers that need `AuthUser` — see `api::current_user`.
pub fn verify_token(secret: &str, token: &str) -> Result<AuthUser> {
    let data = decode::<Claims>(
        token,
        &DecodingKey::from_secret(secret.as_bytes()),
        &Validation::default(),
    )
    .with_context(|| "JWT verification failed")?;
    let id = data
        .claims
        .sub
        .parse::<Uuid>()
        .map_err(|_| anyhow!("invalid uuid in JWT sub"))?;
    Ok(AuthUser {
        id,
        email: data.claims.email,
    })
}

/// Domain error for the auth handlers. Maps onto axum responses so
/// handlers can `?` freely.
#[derive(Debug)]
pub enum AuthError {
    Upstream(String),
    Internal(String),
}

impl IntoResponse for AuthError {
    fn into_response(self) -> Response {
        let (status, msg) = match self {
            AuthError::Upstream(m) => (StatusCode::BAD_GATEWAY, m),
            AuthError::Internal(m) => (StatusCode::INTERNAL_SERVER_ERROR, m),
        };
        (
            status,
            [(header::CONTENT_TYPE, "application/json")],
            serde_json::json!({ "error": msg }).to_string(),
        )
            .into_response()
    }
}

fn urlencode(s: &str) -> String {
    url::form_urlencoded::byte_serialize(s.as_bytes()).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn verify_token_round_trips_a_freshly_issued_jwt() {
        let exp = (Utc::now() + Duration::seconds(60)).timestamp();
        let user_id = Uuid::new_v4();
        let claims = Claims {
            sub: user_id.to_string(),
            email: "x@y.com".into(),
            exp,
        };
        let secret = "test-secret";
        let token = encode(
            &Header::default(),
            &claims,
            &EncodingKey::from_secret(secret.as_bytes()),
        )
        .unwrap();
        let parsed = verify_token(secret, &token).unwrap();
        assert_eq!(parsed.id, user_id);
        assert_eq!(parsed.email, "x@y.com");
    }

    #[test]
    fn verify_token_rejects_wrong_secret() {
        let exp = (Utc::now() + Duration::seconds(60)).timestamp();
        let claims = Claims {
            sub: Uuid::new_v4().to_string(),
            email: "x@y.com".into(),
            exp,
        };
        let token = encode(
            &Header::default(),
            &claims,
            &EncodingKey::from_secret(b"right-secret"),
        )
        .unwrap();
        assert!(verify_token("wrong-secret", &token).is_err());
    }

    #[test]
    fn verify_token_rejects_expired() {
        // jsonwebtoken's default Validation has a 60s leeway; jump well past it.
        let exp = (Utc::now() - Duration::seconds(3600)).timestamp();
        let claims = Claims {
            sub: Uuid::new_v4().to_string(),
            email: "x@y.com".into(),
            exp,
        };
        let secret = "s";
        let token = encode(
            &Header::default(),
            &claims,
            &EncodingKey::from_secret(secret.as_bytes()),
        )
        .unwrap();
        assert!(verify_token(secret, &token).is_err());
    }
}
