# jwc-registry

Package registry server for the [JWC language](../jwc-lang). Sister
service that the `jwc add` / `jwc install` / `jwc publish` commands
talk to once the registry URL is configured (env `JWC_REGISTRY_URL` or
`registry` field in `jwcproj`).

**Status:** v1 — Google OAuth login, multipart tarball upload, version
list / download / delete by owner. No web UI (CLI-only). No payment /
quota. No mirror federation.

## Surface

| Method | Path                                          | Auth | Notes |
|--------|-----------------------------------------------|------|-------|
| GET    | `/healthz`                                    | —    | `{"status":"ok"}` |
| GET    | `/api/v1/auth/google/login`                   | —    | 302 → Google consent |
| GET    | `/api/v1/auth/google/callback?code=…`         | —    | returns `{token, user}` |
| GET    | `/api/v1/me`                                  | Bearer | who am I |
| GET    | `/api/v1/pkg`                                 | —    | list packages |
| GET    | `/api/v1/pkg/:name`                           | —    | package + versions |
| POST   | `/api/v1/pkg/:name/:version`                  | Bearer | multipart `file=@x.tar.gz` |
| GET    | `/api/v1/pkg/:name/:version/download`         | —    | tarball bytes |
| DELETE | `/api/v1/pkg/:name/:version`                  | Bearer | owner only |

## Local dev

```bash
cp .env.example .env  # fill in Google OAuth creds
docker compose up postgres -d
cargo run
```

Then visit `http://localhost:8080/api/v1/auth/google/login` to grab a
token; use it as `Authorization: Bearer <token>` on subsequent calls.

## Tests

```bash
cargo test --lib          # unit tests (no DB)
cargo test                # all tests
```

The integration test in `tests/api_smoke.rs` boots the router against
an in-memory state — no Postgres required.

## Production deploy

`docker-compose.yml` ships both services. For a real deployment:

1. Provision Postgres (RDS / Neon / self-host).
2. Register an OAuth client at Google Cloud Console; set the redirect
   URI to `https://<your-host>/api/v1/auth/google/callback`.
3. `docker compose -f docker-compose.yml up -d registry`.
4. Deploy via `musanna-soft/k8s-gitops` (apps/jwc-registry/) — ArgoCD
   syncs into the `jwc` namespace and fronts `registry-jwc.1kb.uz`
   via the cluster's nginx ingress + cert-manager.

## Schema

Forward-only migrations in `src/db.rs::MIGRATIONS`. The bookkeeping
table `_registry_migrations` records what has been applied; appending
a new entry is the only supported way to evolve the schema.

```
users            (id, google_sub, email, name, created_at)
packages         (id, name, owner_id, created_at)
package_versions (id, package_id, version, sha256, size_bytes, blob_path, uploaded_by, uploaded_at)
```

## Roadmap

See `ROADMAP.md` for the sprint-by-sprint plan (R1 skeleton, R2 OAuth
already in v1; R3 publish CLI integration, R4 ops dashboards, R5
deploy are follow-ups).
