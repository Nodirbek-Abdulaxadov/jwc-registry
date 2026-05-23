# jwc-registry — CLAUDE.md

Sister service to `jwc-lang` (one directory up). Standalone Rust + axum
HTTP server that hosts JWC packages.

## Architecture

- `src/main.rs` — boot wrapper. Parses `Config::from_env`, builds the
  Postgres pool, mounts the router. No request logic.
- `src/api.rs` — Router composition + `FromRequestParts` for `AuthUser`.
- `src/auth.rs` — Google OAuth 2.0 auth-code flow + JWT issuance +
  Bearer verification. **All auth logic lives here.**
- `src/packages.rs` — package + version CRUD + upload/download.
- `src/storage.rs` — sha256-keyed blob store on the local filesystem.
- `src/db.rs` — deadpool-postgres pool + forward-only migrations.
- `src/config.rs` — env-driven runtime config (fail-fast at startup).

## Conventions

- **No unwraps in handlers.** Map every error onto the domain
  `AuthError` / `PackageError` so the JSON response stays consistent.
- **First-publisher-wins ownership.** A new package row is created on
  the first upload; subsequent publishes of the same name must come
  from the owner (`uploaded_by` matches `packages.owner_id`).
- **Forward-only migrations.** Append to `db::MIGRATIONS`; never
  rewrite a previously-shipped entry — already-deployed instances
  won't re-run them.
- **All blob writes go through `BlobStore::put`.** Hash + store +
  return rel path. No code outside `src/storage.rs` should touch
  the filesystem under `storage/`.

## Build / run

```bash
cargo build               # debug
cargo build --release     # production binary
cargo test --lib          # unit tests (no DB; runs in CI)
cargo test                # all (boots router against in-memory state)
cargo run                 # local dev (needs .env from .env.example)
```

## Adding a new endpoint

1. Add the handler in the right module (`auth.rs` / `packages.rs`).
2. Register the route in `api::router`.
3. If the handler needs auth, add `user: AuthUser` to its signature —
   the extractor enforces Bearer JWT verification.
4. Map any new error variants onto `IntoResponse` with a clear status
   + JSON body.

## Schema migration

```rust
// src/db.rs
pub const MIGRATIONS: &[(&str, &str)] = &[
    ("0001_init", "..."),
    ("0002_your_change", "..."), // <- append here
];
```

Run on every boot via `db::run_migrations`. Idempotent.
