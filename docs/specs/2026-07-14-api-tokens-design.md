# API tokens with scopes + quotas — design + plan (roadmap #9)

**Date:** 2026-07-14
**Branch:** `feat/api-tokens` (off main; no merge until reviewed)
**Effort:** medium. Touches the auth path of every `/admin/*` endpoint. Highest
care: the existing `QUARK_ADMIN_TOKEN` behavior must not change.

## Goal

Beyond the single env admin token: named API tokens with scopes (per-permission)
and an optional per-token rate limit, so programmatic use is safe and granular.

## Decisions (locked, user delegated)

- **Scopes** (`Scope` enum): `LinksRead`, `LinksWrite`, `Blocklist`, `Webhooks`,
  `Analytics`, `Full`. `Full` covers every scope (`Scope::covers(required)`).
  The env `QUARK_ADMIN_TOKEN` is always treated as `Full` (superuser) — unchanged.
- **Token**: plaintext `qtok_<32+ base62>` generated on create, shown ONCE.
  Stored as a **SHA-256 hex hash** (tokens are high-entropy; a fast hash is fine,
  no need for password hashing). `ApiToken { id, name, token_hash, scopes:
  Vec<Scope>, rate_limit_per_min: Option<u32>, created }`.
- **`admin_guard` becomes async + scope-aware.** New signature:
  `async fn admin_guard(st, headers, required: Scope) -> Result<(), StatusCode>`.
  Exact status contract (MUST preserve existing behavior):
  1. If `provided == env admin_token` (constant-time, when the env token is set)
     → `Ok` (Full).
  2. Else if `provided` is non-empty: SHA-256 it, look up the api token.
     - found, `scopes` cover `required`, under its rate limit → `Ok`.
     - found but scope insufficient → `403`.
     - found but over its `rate_limit_per_min` → `429`.
     - not found → `401` if env token is set, else `404`.
  3. Else (`provided` empty): `401` if env token is set, else `404`.
  This keeps every current case identical: env token correct → Ok; env token set
  + wrong/empty → 401; env token unset + no matching api token → 404. It only
  ADDS: a valid api token authenticates even when the env token is unset.
- Each `/admin/*` call site passes the scope it needs (list/search/stats →
  `LinksRead` or `Analytics`; create/patch/delete/import/tags-write →
  `LinksWrite`; blocklist → `Blocklist`; webhooks → `Webhooks`). `POST /` create
  keeps `require_admin_for_create` (public unless env token set) UNCHANGED —
  api-token gating of public create is out of scope.
- **Per-token rate limit**: reuse `RateLimiter::check` keyed by `format!("tok:{id}")`
  when the token has `rate_limit_per_min` set; `429` on exceed.
- **Token management endpoints** (require `Full`): `GET /admin/tokens` (list,
  hash/plaintext never returned, only id/name/scopes/rate/created),
  `POST /admin/tokens` `{name, scopes, rate_limit_per_min?}` → `201 {id, token}`
  (plaintext ONCE), `DELETE /admin/tokens/:id` (revoke). Cap at 100 tokens.

## Tasks

### Task 1 — token type + store (LMDB + Postgres) + hashing
Files: `src/auth.rs` (new: `Scope`, `ApiToken`, `generate_token`, `hash_token`,
`Scope::covers`), `src/lib.rs`, `src/store/mod.rs` (trait methods), `lmdb.rs`,
`postgres.rs`, tests.
- `Store`: `list_api_tokens`, `get_api_token_by_hash(hash) -> Option<ApiToken>`,
  `put_api_token`, `delete_api_token(id) -> bool`, `next_api_token_id`. LMDB new
  `api_tokens` db (bump max_dbs); Postgres `api_tokens` table + migration.
- Tests: `hash_token` deterministic; `Scope::covers` (Full covers all, others
  only themselves); store round-trip (LMDB unit + Postgres gated); lookup by hash.

### Task 2 — scope-aware async `admin_guard` + token endpoints + call sites
Files: `src/api.rs`, `src/main.rs` (nothing new in state beyond the store it
already holds), tests.
- Rewrite `admin_guard` to the async, scope-aware contract above; update ALL call
  sites to `admin_guard(&st, &headers, Scope::X).await`. Add the `/admin/tokens`
  CRUD endpoints (require `Full`).
- Tests (`tests/`): **all existing admin tests pass unchanged** (env token
  correct→2xx, wrong→401, unset→404); a `LinksRead` token can GET /admin/links
  but gets `403` on DELETE; a revoked token → 401; over-quota → 429; create
  returns the plaintext once and GET never leaks it.

### Task 3 — UI (tokens page) + docs
Files: `web/src/routes/Tokens.tsx`, Shell nav, router, api/queries/types, i18n;
`docs/API-TOKENS.md` + `.PT_BR.md`, README/ROADMAP.
- Create (name + scope checkboxes + optional rate limit) → show token once with
  copy; list with scopes badges + revoke (confirm). i18n EN+PT. Docs: scopes
  table, `x-admin-token` header usage, curl examples, quota behavior. No em-dashes.

## Global constraints

- **The env `QUARK_ADMIN_TOKEN` behavior is UNCHANGED** (Full superuser); every
  existing admin test must pass without modification. This is the acceptance bar.
- `admin_guard` status contract exactly as specified (404 vs 401 vs 403 vs 429).
- Token plaintext shown once; only the SHA-256 hash is stored; never returned by
  list/get; never logged.
- New fields on persisted structs → `#[serde(default)]` + Postgres migration +
  old-blob regression (the recurring lesson).
- Constant-time compare for the env token; hash lookup for api tokens.
- All code English; UI via i18n (EN + PT-BR); docs EN + `PT_BR`, no em-dashes.
- Rust tests `-j1`; Postgres-gated tests skip cleanly. Stay on `feat/api-tokens`;
  do not merge to main.

## Out of scope

- OAuth / JWT (opaque bearer tokens only).
- Gating the public `POST /` create behind api-token scopes (keeps the
  zero-config open-shortener behavior; env token still locks create).
- Token expiry/rotation UI (revoke + recreate for now).
