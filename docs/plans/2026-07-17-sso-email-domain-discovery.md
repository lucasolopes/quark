# SSO discovery by email domain (LUC-57) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development. Steps use checkbox (`- [ ]`) syntax.

**Goal:** A user of an SSO tenant types their email on the central login; if the email domain is a verified SSO domain of a tenant with OIDC, they're routed straight to that tenant's login (`/admin/login?org=<slug>`). No slug typed, no per-tenant host. Home Realm Discovery.

**Architecture:** A new `sso_email_domains` table (dedicated, kept off the redirect hot path) maps a verified email domain → tenant, verified by DNS TXT reusing the P3 `Dns` seam. A public rate-limited `GET /admin/sso/discover?email=` returns the org slug for a verified match. `Login.tsx` gains an email-first step that calls discover and routes to the tenant SSO or falls back to the shared login. Admin CRUD + UI manage the domains. Cloud-only, opt-in; reuses the existing `?org=` login (LUC-53) and per-tenant OIDC (P2d/P2e).

**Tech Stack:** Rust (axum, sqlx), React+TypeScript (Vitest). `src/codec.rs`/`src/permute.rs` UNTOUCHED.

## Global Constraints
- Code + copy English (source); user-facing strings via i18n in BOTH `pt-BR` and `en`. avoid-ai-writing for prose.
- **Cloud-only + opt-in:** everything gated on `st.multi_tenant` (cloud). OSS = the SSO-domain surface does not exist (endpoints 404, login email step absent). OSS/model-A behavior byte-for-byte unchanged; parity tests prove it.
- DNS-TXT verification is MANDATORY before a domain routes (anti-hijack). `domain` is UNIQUE global (one tenant per email domain). Only a tenant WITH an `oidc_config` may add/route SSO domains.
- Discovery must be anti-enumeration: uniform `200` response (body differs), rate-limited per IP, and it must NEVER leak `tenant_id` (only the slug, already usable via `?org=`).
- `src/codec.rs`/`src/permute.rs` MUST NOT be touched. The redirect hot path (`get_domain_by_host`, `resolve_*`) MUST NOT be touched — the new table is separate from `domains`.
- Postgres-gated tests via `QUARK_TEST_DATABASE_URL` (local PG may be `postgres://quark:quark@localhost:5432/quark`), verified as NON-SUPERUSER in cloud. cargo `-j1` / `CARGO_BUILD_JOBS=1`. NO `CREATE INDEX CONCURRENTLY`. LNK1104 = stale locked test exe → kill + retry. In Bash: `export PATH="$HOME/.cargo/bin:$PATH"`.
- Build/fmt/clippy (`--all-targets -- -D warnings`) clean; web `tsc --noEmit` + `oxlint` clean.

## Seams (verified)
- `domains` table + store (`src/store/postgres.rs`): `put_domain`, `get_domain` (tenant-scoped), `get_domain_by_host` (bare), `list_domains`, `set_domain_status`; DDL at ~`:631`; `DomainStatus` enum {Pending, Verified}. MIRROR these shapes for `sso_email_domains` — do NOT reuse the `domains` table.
- `Dns` seam (`src/dns.rs`, `AppState.dns: Arc<dyn Dns>`); `admin_domains_verify` in `src/api.rs` does the TXT lookup + status flip — MIRROR for the SSO verify handler.
- `TENANT_OWNED_TABLES` + `NOT_FORCED` lists (`src/store/postgres.rs` ~`:91`): add the new table (NOT_FORCED — bare lookup by domain before the tenant is known).
- `admin_guard(&st, &headers, Scope::Full)` returns a Principal with `p.tenant`; the ratelimiter is `st.ratelimiter.check(&ip, now())`. Cloud gate: `st.multi_tenant`.
- Per-tenant OIDC: `get_oidc_config_bare(tenant) -> Option<..>` marks SSO tenants.
- `GET /admin/login?org=<slug>` (existing) is the redirect target. `Login.tsx` already reads `?org=` (LUC-53); `oidcLoginUrl(org?)` + `api` client in `web/src/lib/api.ts`.
- Route registration + CORS list near `src/api.rs:4529`.

## File Structure
- `src/tenant.rs` or a small `src/sso.rs`: `SsoEmailDomain` struct + helpers (email→domain parse, domain normalization).
- `src/store/postgres.rs` + `src/store/mod.rs` (trait) + `src/store/lmdb.rs` (OSS stub): the store methods.
- `src/api.rs`: discovery endpoint + admin CRUD/verify handlers + routes.
- `web/src/lib/api.ts` + `web/src/routes/Login.tsx` + i18n: email-first login.
- `web/src/routes/` (Settings area): SSO-domains admin UI.

---

### Task 1: `sso_email_domains` table + store

**Files:** Modify `src/tenant.rs` (or new `src/sso.rs`), `src/store/mod.rs` (trait), `src/store/postgres.rs`, `src/store/lmdb.rs`; Test `tests/` (new `sso_domains_it.rs`, PG-gated).

**Produces (Rust signatures later tasks use):**
- `struct SsoEmailDomain { id: u64, tenant_id: TenantId, domain: String, token: String, status: DomainStatus, created: u64, verified_at: Option<u64> }` (reuse the existing `DomainStatus`).
- `fn normalize_email_domain(email: &str) -> Option<String>` — lowercase the part after the last `@`; `None` for malformed.
- Store trait methods: `put_sso_domain(&SsoEmailDomain)`, `get_sso_domain_bare(&str) -> Option<SsoEmailDomain>`, `list_sso_domains(TenantId) -> Vec<..>`, `set_sso_domain_status(TenantId, id, DomainStatus, Option<u64>)`, `delete_sso_domain(TenantId, id)`, `next_sso_domain_id() -> u64`.

**Steps:**
- [ ] Read the `domains` DDL, `DomainStatus`, and its store methods in `src/store/postgres.rs`; and `normalize`/host handling, to mirror exactly.
- [ ] Write failing PG-gated tests (`tests/sso_domains_it.rs`, guard on `QUARK_TEST_DATABASE_URL`): put a pending domain → `get_sso_domain_bare` returns it; a second tenant putting the SAME `domain` → UNIQUE conflict (store error); `set_sso_domain_status` → verified; `list_sso_domains` tenant-scoped (tenant A doesn't see B's); `delete`; `normalize_email_domain("a@ACME.com")=="acme.com"`, malformed → None. Run, confirm fail.
- [ ] Add the table to `init_schema`: `CREATE TABLE IF NOT EXISTS sso_email_domains (id BIGINT PRIMARY KEY, tenant_id BIGINT NOT NULL DEFAULT 0, domain TEXT NOT NULL UNIQUE, token TEXT NOT NULL, status TEXT NOT NULL, created BIGINT NOT NULL, verified_at BIGINT)`. Add it to `TENANT_OWNED_TABLES` (generic tenant_id) and `NOT_FORCED` (bare lookup). NO CONCURRENTLY. Implement the struct, `normalize_email_domain`, and the store methods (mirror `domains`; `next_sso_domain_id` mirrors `next_domain_id`). LMDB: minimal stubs (OSS never uses SSO domains — return empty/None/unsupported consistent with other cloud-only store methods).
- [ ] Run tests; build/fmt/lib + gated. Commit `feat(store): sso_email_domains table + store (verified email-domain -> tenant)`.

---

### Task 2: admin CRUD + DNS-TXT verify endpoints

**Files:** Modify `src/api.rs` (handlers + routes); Test `tests/sso_domains_it.rs` (extend, PG-gated).

**Produces:** `GET /admin/sso-domains`, `POST /admin/sso-domains` (body `{domain}`), `DELETE /admin/sso-domains/:id`, `POST /admin/sso-domains/:id/verify`.

**Steps:**
- [ ] Read `admin_domains_verify` + the domains CRUD handlers + how `st.dns` is called + the TXT record convention, to mirror.
- [ ] Write failing tests (PG-gated, mock `Dns`): POST a domain as `Scope::Full` admin of a tenant WITH `oidc_config` → 201 pending, returns the TXT token; POST when the tenant has NO `oidc_config` → 409/400 (SSO not configured); POST an already-claimed domain → 409; `verify` with a matching TXT (mock Dns returns the token) → verified; `verify` with no/wrong TXT → stays pending (e.g. 422/200-pending per the domains convention — match it); list is tenant-scoped; delete works; all endpoints 404 in OSS (`!multi_tenant`); a Viewer/insufficient scope → 403.
- [ ] Run, confirm fail.
- [ ] Implement the handlers: all cloud-only (`if !st.multi_tenant → 404`), `admin_guard(Full)`, tenant-scoped on `p.tenant`. `POST` gates on `get_oidc_config_bare(p.tenant).is_some()`. `verify` mirrors `admin_domains_verify` (build the expected record `_quark-sso.<domain>`, TXT lookup via `st.dns`, flip status). Register routes near the other `/admin/*`.
- [ ] Run tests; build/fmt/lib + gated. Commit `feat(api): admin CRUD + DNS-TXT verify for SSO email domains (cloud-only, Full)`.

---

### Task 3: public discovery endpoint

**Files:** Modify `src/api.rs` (handler + route); Test `tests/sso_domains_it.rs` (extend).

**Produces:** `GET /admin/sso/discover?email=<email>` → `200 { "org"?: "<slug>" }` (uniform).

**Steps:**
- [ ] Write failing tests (PG-gated): a verified domain of a tenant WITH `oidc_config` → `200 {"org":"<slug>"}`; a pending domain → `200 {}`; unknown domain → `200 {}`; a verified domain whose tenant LOST its `oidc_config` → `200 {}`; malformed email → `200 {}`; response NEVER contains `tenant_id`; OSS → 404; the per-IP rate limit trips after the configured burst (mirror how other endpoints assert rate-limit). Run, confirm fail.
- [ ] Implement: cloud-only (`!multi_tenant → 404`); rate-limit by IP (`st.ratelimiter.check(&ip, now())` → 429 on trip, or the uniform 200 with empty body if the project prefers not to distinguish — match the anti-enumeration intent, but a 429 for rate-limit is fine and expected). `normalize_email_domain(email)` → `get_sso_domain_bare(domain)` → if `status==Verified` AND `get_oidc_config_bare(tenant).is_some()` AND the tenant resolves to a slug → `{org: slug}`; else `{}`. Resolve the slug via the existing tenant lookup (`get_tenant`). Never serialize `tenant_id`.
- [ ] Run tests; build/fmt/lib + gated. Commit `feat(api): GET /admin/sso/discover — email-domain home-realm discovery (anti-enumeration)`.

---

### Task 4: email-first login (frontend)

**Files:** Modify `web/src/lib/api.ts`, `web/src/routes/Login.tsx`, `web/src/i18n/pt-BR.ts`, `web/src/i18n/en.ts`; Test `web/src/lib/api.test.ts`, `web/src/routes/Login.test.tsx`.

**Steps:**
- [ ] Read `Login.tsx` (current token + shared-OIDC button, the `?org=` handling from LUC-53) and `api.ts` (`oidcLoginUrl`, `req`, `BASE`).
- [ ] Add `api.discoverSso(email: string): Promise<{ org?: string }>` → `GET ${BASE}/admin/sso/discover?email=${encodeURIComponent(email)}`. Test it in `api.test.ts` (returns `{org}` / `{}`).
- [ ] Write failing `Login.test.tsx` tests: with no `?org=` and `oidcEnabled`, an email step is shown; submitting an email whose discover returns `{org:"acme"}` → sets `window.location.href` to `oidcLoginUrl("acme")`; discover returns `{}` → falls back to the shared provider button + token field (no redirect); with `?org=acme` in the URL, the email step is skipped and "Entrar em acme" shows (LUC-53 regression); with `oidc_enabled=false` (OSS), no email step. Run, confirm fail.
- [ ] Implement in `Login.tsx`: an email-first stage (email input + "Continue") shown when `oidcEnabled && !org`. On submit → `discoverSso(email)`; `org` present → `window.location.href = oidcLoginUrl(org)`; absent → reveal/keep the shared login (provider button + token). `?org=` in the URL keeps precedence (skip the email stage). Token field remains reachable. Add i18n keys (`login.emailLabel`, `login.continue`, `login.emailHint` or similar) to BOTH locales.
- [ ] Run the web suite (`npx vitest run` inside `web/`) + `tsc`/`oxlint`; keep `Login.test.tsx` green. Commit `feat(web): email-first login routes SSO users to their org via discovery`.

---

### Task 5: admin UI for SSO email domains

**Files:** Modify/create under `web/src/` (a Settings route/section), `web/src/lib/api.ts` (CRUD calls), `web/src/lib/queries.ts`, i18n; Test the new component + `api.test.ts`.

**Steps:**
- [ ] Look for an existing custom-domains admin UI (P3) to mirror; if present, copy its structure. Read how Settings routes/components are organized.
- [ ] Add `api` methods: `listSsoDomains()`, `createSsoDomain(domain)`, `verifySsoDomain(id)`, `deleteSsoDomain(id)` (+ `queries.ts` hooks). Test in `api.test.ts` (URL/method/body).
- [ ] Write failing component tests: the section lists domains with status, shows the TXT record to create for a pending domain, a verify button calls `verifySsoDomain`, add/remove work; the section is only rendered for a tenant with SSO configured (mirror how other cloud-only sections gate — e.g. on `me()` fields). Run, confirm fail.
- [ ] Implement the UI (mirror the P3 custom-domains section if it exists; else a simple table + add form + per-row verify/delete). i18n keys in BOTH locales. Gate visibility to cloud + SSO-configured tenants.
- [ ] Run the web suite + `tsc`/`oxlint`. Commit `feat(web): admin UI to add/verify/remove SSO email domains`.

## Verification (whole-plan)
- Store: verified email domain → tenant; UNIQUE domain; tenant-scoped list; PG NON-SUPERUSER.
- Verify: TXT match flips to verified (mock Dns); only-with-oidc_config; cloud-only.
- Discovery: verified+oidc → `{org}`; everything else → `{}`; no `tenant_id` leak; rate-limited; OSS 404.
- Login: email → discover → route to tenant SSO; fallback to shared; `?org=` precedence (LUC-53 intact); OSS no email step.
- Admin UI: add/verify/remove, cloud+SSO-gated.
- Full Rust suite (`-j1`) + web suite green; clippy `--all-targets -D warnings` + `tsc` + `oxlint` clean. OSS/model-A parity intact. Opus review on Tasks 2+3 (auth/anti-enumeration). Then whole-branch review before merge.
