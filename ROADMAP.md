# jwc-registry — Roadmap

Sprint-by-sprint plan that grew out of the jwc-lang Sprint 9-11
"Registry server" lane. Each sprint is sized for one focused session.

## Sprint Tracker

| # | Sprint | Status | Notes |
|---|--------|--------|-------|
| R1 | Skeleton + axum + healthz | ✅ | Cargo workspace, Dockerfile, docker-compose, `/healthz`, env-driven `Config`. |
| R2 | Google OAuth + JWT sessions | ✅ | Auth-code flow, userinfo upsert, signed JWT, Bearer extractor. |
| R3 | Package model + upload/download/list/delete | ✅ | Postgres schema, blob store, multipart upload, owner-only delete. |
| R4 | jwc-lang client integration (`jwc publish` / `jwc login`) | ⬜ | Wire `jwc-lang/src/cmd/pkg.rs` to the new endpoints; ship `~/.jwc/credentials.json`. |
| R5 | Operations (rate limit, OTel, backup) | ⬜ | `governor` per-IP, `tracing-opentelemetry`, pg-dump cron, Grafana board. |
| R6 | Production deploy on registry-jwc.1kb.uz | ⏳ | Deployed via musanna-soft/k8s-gitops (apps/jwc-registry) + ArgoCD; DNS pointing to cluster ingress, Let's Encrypt via cert-manager. |

## Why Google-only auth (v1)

Less code than rolling our own email/password (no flow for password
reset, lockout, email verification, etc.) and gives every contributor
a recoverable account by default. We can add additional providers
later by extending `src/auth.rs` — the JWT shape stays the same so the
client doesn't need to change.

## Out of scope for v1

- Web UI (CLI-only registry; package discovery via `GET /api/v1/pkg`).
- Yanking / unpublishing across mirrors.
- Mirror federation.
- Quota / billing.
- Owner transfer between users (delete + re-publish workflow for now).
