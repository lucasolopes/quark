# Deep linking (app-association hosting) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: superpowers:subagent-driven-development. Steps use `- [ ]`.

**Goal:** Host the iOS `apple-app-site-association` and Android `assetlinks.json` documents on the redirect domain, editable from the admin panel, persisted via the pluggable Store.

**Architecture:** A two-key document Store (raw JSON in, verbatim out). Public unauthenticated GET routes at the OS well-known paths; admin-guarded CRUD to set them. Frontend "App Links" editor page. Deep-link *redirect* logic is out of scope (deferred).

**Tech Stack:** Rust (axum, heed/LMDB, sqlx/Postgres), React + TS + Vitest.

## Global Constraints

- Code in English; no inline `//` comments (keep `///`/`//!`); UI i18n EN + PT-BR; docs EN + `.PT_BR.md`.
- `QUARK_ADMIN_TOKEN` behavior unchanged; GET well-known routes are public (no auth).
- Redirect hot path pays nothing (these are separate routes, resolved before any code lookup).
- Store the raw JSON verbatim; validate only: allowed name, parseable JSON, body ≤ 64 KiB (65536 bytes).
- Allowed document names (exact): `apple-app-site-association`, `assetlinks.json`.
- No merge to main. avoid-ai-writing on all prose. Rust tests run with `CARGO_BUILD_JOBS=1` / `-j1`; kill any stray `quark.exe` before a release build.
- Postgres tests gated by `QUARK_TEST_DATABASE_URL`.

---

### Task 1: Store — well-known document persistence

**Files:**
- Modify: `src/store/mod.rs` (trait `Store`, ~line 60–90: add 3 methods)
- Modify: `src/store/lmdb.rs` (add `wellknown: Database<Str, Str>`; `MAX_DBS` 6→7 at line 19; create db at ~line 83; impl 3 methods)
- Modify: `src/store/postgres.rs` (migration + impl 3 methods)
- Test: inline `#[cfg(test)]` in `lmdb.rs` (round-trip) and the gated Postgres integration test file used by the other store methods

**Interfaces:**
- Produces (on `trait Store`):
  - `async fn get_wellknown(&self, name: &str) -> Result<Option<String>, StoreError>`
  - `async fn put_wellknown(&self, name: &str, body: &str) -> Result<(), StoreError>`
  - `async fn delete_wellknown(&self, name: &str) -> Result<(), StoreError>`

- [ ] **Step 1: Failing LMDB round-trip test.** In `src/store/lmdb.rs` tests, open a temp store, assert `get_wellknown("assetlinks.json")` is `None`, `put_wellknown` a body, `get_wellknown` returns it, `delete_wellknown`, then `get_wellknown` is `None` again.
- [ ] **Step 2: Run it, verify it fails** (`CARGO_BUILD_JOBS=1 cargo test -j1 -p quark --lib store::lmdb` — fails: method not found).
- [ ] **Step 3: Add the 3 methods to `trait Store`** in `src/store/mod.rs` (signatures above).
- [ ] **Step 4: LMDB impl.** Add field `wellknown: Database<Str, Str>`, bump `MAX_DBS` to 7, `create_database(&mut wtxn, Some("wellknown"))`, wire into the struct constructor. Implement get (read_txn), put (write_txn `.put`), delete (write_txn `.delete`, treat missing key as Ok).
- [ ] **Step 5: Postgres impl.** In the migration function add `CREATE TABLE IF NOT EXISTS wellknown_documents (name TEXT PRIMARY KEY, body TEXT NOT NULL)`. Implement get (`SELECT body ... WHERE name=$1`), put (`INSERT ... ON CONFLICT (name) DO UPDATE SET body=EXCLUDED.body`), delete (`DELETE ... WHERE name=$1`). Mirror the gated round-trip test used by other Postgres store methods.
- [ ] **Step 6: Run** `CARGO_BUILD_JOBS=1 cargo test -j1 -p quark --lib` — passes. Then `cargo fmt` + `cargo clippy -j1 --all-targets -- -D warnings`.
- [ ] **Step 7: Commit** `feat(deep-linking): store for well-known app-association documents`.

### Task 2: HTTP — serve well-known files + admin CRUD

**Files:**
- Modify: `src/api.rs` (3 GET handlers + 3 admin handlers; register 6 routes in `router_with_cors` ~line 618–634)
- Test: `tests/api_it.rs` (integration, mirroring existing admin-endpoint tests)

**Interfaces:**
- Consumes: `Store::{get,put,delete}_wellknown` (Task 1); `admin_guard(&st, &headers) -> Result<(), StatusCode>`; handler shape `State<Arc<AppState>>, headers: HeaderMap`.
- Constants: `const WELLKNOWN_NAMES: [&str; 2] = ["apple-app-site-association", "assetlinks.json"]; const WELLKNOWN_MAX: usize = 65536;`

- [ ] **Step 1: Failing API tests** in `tests/api_it.rs`: (a) PUT `/admin/wellknown/assetlinks.json` with a valid JSON body + admin header → 200; GET `/.well-known/assetlinks.json` → 200, `content-type: application/json`, body equals what was PUT. (b) GET an unset `/.well-known/apple-app-site-association` → 404. (c) PUT non-JSON body → 400. (d) PUT `/admin/wellknown/bogus` → 404. (e) PUT >64 KiB → 400. (f) PUT without admin token → 401. (g) after PUT of AASA, GET the legacy root `/apple-app-site-association` returns the same body.
- [ ] **Step 2: Run, verify they fail** (`CARGO_BUILD_JOBS=1 cargo test -j1 -p quark --test api_it wellknown` — routes 404/unrouted).
- [ ] **Step 3: Serving handlers.** `wellknown_aasa` and `wellknown_assetlinks`: read the doc; on `Some(body)` return `([(CONTENT_TYPE, "application/json")], body)` 200; `None` → 404; `Err` → 503. No auth.
- [ ] **Step 4: Admin handlers.** `admin_wellknown_get/put/delete` on `/admin/wellknown/:name`: `admin_guard` first; reject `:name` not in `WELLKNOWN_NAMES` with 404. PUT: reject body > `WELLKNOWN_MAX` (400), reject non-JSON via `serde_json::from_str::<serde_json::Value>` (400), else `put_wellknown` → 200. GET → stored body or 404. DELETE → `delete_wellknown` → 204/200.
- [ ] **Step 5: Register routes** in `router_with_cors` before `.with_state`: `.route("/.well-known/apple-app-site-association", get(wellknown_aasa))`, `.route("/apple-app-site-association", get(wellknown_aasa))`, `.route("/.well-known/assetlinks.json", get(wellknown_assetlinks))`, `.route("/admin/wellknown/:name", get(admin_wellknown_get).put(admin_wellknown_put).delete(admin_wellknown_delete))`.
- [ ] **Step 6: Run** `CARGO_BUILD_JOBS=1 cargo test -j1 -p quark --test api_it` — passes. `cargo fmt` + clippy `-D warnings`.
- [ ] **Step 7: Commit** `feat(deep-linking): serve AASA + assetlinks.json, admin CRUD`.

### Task 3: Frontend — "App Links" editor page

**Files:**
- Create: `web/src/routes/AppLinks.tsx`
- Modify: `web/src/lib/api.ts`, `web/src/lib/queries.ts`, `web/src/lib/types.ts`, the Shell nav, `web/src/i18n/en.ts` + `web/src/i18n/pt-BR.ts`, the router registration
- Test: `web/src/routes/AppLinks.test.tsx` (Vitest)

**Interfaces:**
- Consumes: admin endpoints from Task 2 (`GET/PUT/DELETE /admin/wellknown/:name`).

- [ ] **Step 1: Failing Vitest** — render `AppLinks`, type invalid JSON into the AASA editor, assert an "invalid JSON" message shows and Save is disabled; type valid JSON, assert Save enabled.
- [ ] **Step 2: Run, verify fail** (`cd web && npx vitest run AppLinks`).
- [ ] **Step 3: api.ts + queries.ts** — `getWellknown(name)`, `putWellknown(name, body)`, `deleteWellknown(name)`; a query hook per document + mutation.
- [ ] **Step 4: AppLinks.tsx** — two editor cards (AASA, assetlinks.json): load current on mount, textarea, live JSON validity (parse on change), Save (disabled when invalid/empty), Clear (delete), success/error toast. Short HTTPS note. All copy via i18n keys.
- [ ] **Step 5: i18n** — add keys to `en.ts` and mirror in `pt-BR.ts` (parity). Add nav entry + route.
- [ ] **Step 6: Run** `cd web && npx vitest run && npx tsc --noEmit && npm run lint && npm run build` — all green.
- [ ] **Step 7: Commit** `feat(deep-linking): App Links panel page`.

### Task 4: Docs + README + ROADMAP

**Files:**
- Create: `docs/DEEP-LINKING.md`, `docs/DEEP-LINKING.PT_BR.md`
- Modify: `README.md` (+ `README.PT_BR.md` if present), `docs/ROADMAP.md` (+ PT_BR)

- [ ] **Step 1** Write `docs/DEEP-LINKING.md`: what AASA + assetlinks.json are, how iOS/Android fetch them (well-known paths, HTTPS, no redirect), how to produce them (point at Apple/Google docs), how to set them in the panel, and an explicit "device-aware redirect (open the app) is a deferred follow-up" note. Prose through avoid-ai-writing (no em-dashes, no AI-isms).
- [ ] **Step 2** Mirror to `docs/DEEP-LINKING.PT_BR.md` with the EN/PT header links.
- [ ] **Step 3** Add a README docs-list link; move ROADMAP #20 to "core done (app-association hosting), device-aware redirect follow-up".
- [ ] **Step 4: Commit** `docs(deep-linking): AASA/assetlinks hosting guide (EN+PT_BR)`.

## Self-review

- Spec coverage: Store (T1), serving + admin + validation + routing precedence + 404 (T2), panel (T3), docs/ROADMAP (T4). Non-goals (device-aware redirect) explicitly excluded.
- Types consistent: `get/put/delete_wellknown` signatures identical across mod.rs/lmdb.rs/postgres.rs and the plan; `WELLKNOWN_NAMES`/`WELLKNOWN_MAX` defined once in api.rs.
- No placeholders: each step names the file, the assertion, and the command.
