## Project Documentation Context

quark is a single-binary URL shortener. Backend is Rust (axum + tokio); the
admin panel is a React + TypeScript + Vite SPA under `web/`. Storage, cache,
analytics, and rate-limiting are pluggable: embedded defaults (LMDB, in-memory)
with opt-in production backends (Postgres, Valkey/Redis, ClickHouse).

### Code Structure
- HTTP handlers and the router live in `src/api/`, a directory module split by
  area. `mod.rs` holds `AppState`, the shared imports, and re-exports; handlers
  are grouped into `links.rs` and `links_admin.rs` (the `/`, `/:code`, and admin
  link CRUD), `guard.rs` (admin auth), `oidc_login.rs`, `tenants.rs`,
  `domains.rs`, `sso_domains.rs`, `invites.rs`, `sheets.rs`, `webhooks_api.rs`,
  and `router.rs` (`router()` / `router_with_cors()`). Submodules use
  `use super::*;` over a flat glob re-export in `mod.rs`, so the internal
  namespace stays flat and the public surface (`AppState`, `router`, ...) is
  reachable at `quark::api::`.
- Request/response types are serde structs defined inline in the relevant
  `src/api/*.rs` submodule (e.g. `CreateReq`, `CreateResp` in `links.rs`).
  Persisted domain types (`Record`, `Rule`, `Variant`, ...) live in
  `src/store/mod.rs`.
- Storage: `src/store/` — the `Store` trait in `mod.rs`, backends `lmdb.rs`
  (default, embedded) and `postgres.rs` (shared).
- Cache: `src/cache/` — `mod.rs` (L1 moka + optional L2 tier), `valkey.rs`.
- Analytics: `src/analytics/` — `mod.rs` (`ClickEvent`, `Aggregates`, the
  channel worker, the `AnalyticsSink` trait), `clickhouse.rs`.
- Webhooks: `src/webhooks/` — `mod.rs` (types, Standard Webhooks signing),
  `delivery.rs` (dispatcher + delivery worker).
- Abuse / cross-cutting guards (the closest thing to middleware): `src/abuse/`
  — `ratelimit.rs` and `mod.rs` (SSRF `is_internal_host`,
  `extract_host`). Admin auth is in `src/api/guard.rs` (`admin_guard`,
  `require_admin_for_create`); API tokens and scopes are in `src/auth.rs`.
- Other modules: `src/pixel.rs` (conversion forwarding), `src/import.rs`,
  the `arxid` crate reexported as `quark::{permute, codec}` in `src/lib.rs`
  (keyed Feistel code generation + base62; extracted from this repo),
  `src/invalidate.rs` (cross-node pub/sub invalidation), `src/main.rs`,
  `src/lib.rs`.
- Tests: integration tests are `tests/*_it.rs` (e.g. `api_it.rs`,
  `webhooks_api_it.rs`, `tokens_api_it.rs`); unit tests are inline
  `#[cfg(test)]` modules. Postgres / Valkey / ClickHouse integration tests are
  gated behind env vars (`QUARK_TEST_DATABASE_URL`, `QUARK_TEST_VALKEY_URL`,
  ...). Integration tests build their `AppState` through the shared
  `tests/common/mod.rs` `TestState` builder (defaults to the OSS single-tenant
  shape, fluent setters per field) rather than a hand-rolled struct literal.
  Frontend tests are `web/src/**/*.test.tsx` (Vitest).

### Open core (LUC-19)
- Everything is AGPL-3.0-only EXCEPT `src/ee/` and `web/src/ee/`, which are
  under the quark Enterprise Edition License (`src/ee/LICENSE`). Deleting both
  directories must leave a buildable, passing core; CI enforces it in the
  `community-only` job. Never make core code depend on `src/ee/`.
- The server side lives behind the non-default cargo feature `ee`
  (`cargo build --features ee`); the panel behind the `@ee` alias, which
  `vite.config.ts` points at `web/src/ee/` when `VITE_QUARK_EE=1` and at the
  inert `web/src/lib/ee-stub.tsx` otherwise. The stub is the type contract the
  real barrel must satisfy.
- What belongs in `ee`: administering OTHER people's accounts (workspaces,
  invites, per-tenant IdP, Keycloak realm provisioning, multiple verified
  domains, billing). What stays core: anything one organization uses for
  itself, anything on the redirect hot path, and every type named by the
  `Store` trait. The full rule and the file-by-file inventory are in
  `docs/specs/2026-08-03-luc19-open-core-design.md` and
  `docs/research/2026-08-03-luc19-inventario-oss-ee.md`.
- Modules in `src/ee/api/` reach the core namespace with `use crate::api::*`
  (the reexports in `src/api/mod.rs` are `pub(crate)`), then `use super::*` for
  their siblings. EE routes are mounted at ONE injection point,
  `crate::ee::api::mount`, called from `router_with_cors`; never scatter
  `#[cfg(feature = "ee")]` across individual routes. The EE boot is likewise one
  call, `crate::ee::boot`, from `main.rs`.
- Gated test binaries that only exercise EE routes carry
  `#![cfg(feature = "ee")]`; panel tests for EE screens live in `web/src/ee/`
  and run via `npm run test:ee`.

### Documentation Format
- Project docs live in `docs/` as Markdown files.
- Each user-facing feature has an English doc plus a `.PT_BR.md` twin
  (e.g. `docs/WEBHOOKS.md` and `docs/WEBHOOKS.PT_BR.md`). Both start with the
  language-switch header line: `**English** · [Português](X.PT_BR.md)` (and the
  mirror on the PT_BR file).
- Design specs go in `docs/specs/`, implementation plans in `docs/plans/`,
  research and audits in `docs/research/`.
- Prose follows the avoid-ai-writing rules: no em-dashes, plain direct
  technical English (and natural pt-BR on the PT_BR twin).
