## Project Documentation Context

quark is a single-binary URL shortener. Backend is Rust (axum + tokio); the
admin panel is a React + TypeScript + Vite SPA under `web/`. Storage, cache,
analytics, and rate-limiting are pluggable: embedded defaults (LMDB, in-memory)
with opt-in production backends (Postgres, Valkey/Redis, ClickHouse).

### Code Structure
- HTTP handlers and the router live in a single module `src/api.rs`
  (all `/`, `/:code`, `/admin/*` handlers, `router()` / `router_with_cors()`,
  and `AppState`). There is no per-domain handler file; handlers are grouped by
  comment within `src/api.rs`.
- Request/response types are serde structs defined inline in `src/api.rs`
  (e.g. `CreateReq`, `CreateResp`). Persisted domain types (`Record`, `Rule`,
  `Variant`, ...) live in `src/store/mod.rs`.
- Storage: `src/store/` — the `Store` trait in `mod.rs`, backends `lmdb.rs`
  (default, embedded) and `postgres.rs` (shared).
- Cache: `src/cache/` — `mod.rs` (L1 moka + optional L2 tier), `valkey.rs`.
- Analytics: `src/analytics/` — `mod.rs` (`ClickEvent`, `Aggregates`, the
  channel worker, the `AnalyticsSink` trait), `clickhouse.rs`.
- Webhooks: `src/webhooks/` — `mod.rs` (types, Standard Webhooks signing),
  `delivery.rs` (dispatcher + delivery worker).
- Abuse / cross-cutting guards (the closest thing to middleware): `src/abuse/`
  — `ratelimit.rs` and `mod.rs` (SSRF `is_internal_host`,
  `extract_host`). Admin auth is in `src/api.rs` (`admin_guard`,
  `require_admin_for_create`); API tokens and scopes are in `src/auth.rs`.
- Other modules: `src/pixel.rs` (conversion forwarding), `src/import.rs`,
  `src/permute.rs` (keyed Feistel code generation), `src/codec.rs` (base62),
  `src/invalidate.rs` (cross-node pub/sub invalidation), `src/main.rs`,
  `src/lib.rs`.
- Tests: integration tests are `tests/*_it.rs` (e.g. `api_it.rs`,
  `webhooks_api_it.rs`, `tokens_api_it.rs`); unit tests are inline
  `#[cfg(test)]` modules. Postgres / Valkey / ClickHouse integration tests are
  gated behind env vars (`QUARK_TEST_DATABASE_URL`, `QUARK_TEST_VALKEY_URL`,
  ...). Frontend tests are `web/src/**/*.test.tsx` (Vitest).

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
