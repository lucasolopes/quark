# Google Sheets Integration Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** a native OAuth Google Sheets connector that mirrors the link catalog into a spreadsheet the operator owns, synced on demand and on an optional schedule.

**Architecture:** a dedicated `src/sheets/` module holds the Google OAuth (Authorization Code, offline) and the sync orchestration; a `SheetsApi` trait is the HTTP seam so the sync logic tests with a mock and no real credentials. A single `SheetsConnection` record persists in the Store (LMDB + Postgres). The panel's Extensions card drives connect/disconnect/sync. Reuses the patterns of `src/oidc.rs` (reqwest client, code exchange) and `src/pixel.rs` (secret masking) without sharing their config types.

**Tech Stack:** Rust 2021, axum, tokio, reqwest (already a dependency via OIDC), sqlx (Postgres), heed (LMDB); React 19 + Tailwind v4 + @tanstack/react-query on the frontend.

## Global Constraints

- Code in English; no unexplained abbreviations.
- UI is i18n EN + PT-BR; every new key added to BOTH `web/src/i18n/en.ts` and `pt-BR.ts` (TS enforces key parity at compile time).
- Docs in EN + PT-BR (`docs/SHEETS.md` + `docs/SHEETS.PT_BR.md`).
- The redirect hot path (`GET /:code`) pays nothing for this feature.
- The `refresh_token` is masked in every API response and NEVER logged.
- Google endpoints are fixed hosts (`oauth2.googleapis.com`, `sheets.googleapis.com`) — no user-controlled URL, so no SSRF guard is needed (unlike webhook destinations).
- Scheduled sync is lease-coordinated (Postgres) so it is safe on every replica; the single-node LMDB lease always returns `true`.
- OAuth scope is exactly `https://www.googleapis.com/auth/drive.file`.
- Opt-in: the connector is OFF unless `QUARK_SHEETS_CLIENT_ID` + `_SECRET` + `_REDIRECT_URL` are all set.
- Rust build/test in this environment: `export PATH="$HOME/.cargo/bin:$PATH"`; kill the running binary first (`powershell -Command "Get-Process quark -ErrorAction SilentlyContinue | Stop-Process -Force"`); always `CARGO_BUILD_JOBS=1 cargo <cmd> -j1`.
- `api_it` uses LMDB — do NOT set `QUARK_TEST_DATABASE_URL` for it. Postgres-gated store tests read `QUARK_TEST_DATABASE_URL=postgres://quark:quark@127.0.0.1:5432/quark`.
- No merge to `main` until reviewed. Branch: `feat/sheets`.

## File Structure

- `src/sheets/mod.rs` (new) — `SheetsConfig`, `SheetsConnection`, `SyncStatus`, OAuth URL/exchange/refresh, `sync()` orchestration + row building.
- `src/sheets/client.rs` (new) — `SheetsApi` trait + real reqwest impl (`GoogleSheetsApi`).
- `src/lib.rs` (modify) — `pub mod sheets;`.
- `src/store/mod.rs` (modify) — 3 trait methods + a lease method; the `SheetsConnection` type is re-exported from `sheets`.
- `src/store/lmdb.rs` (modify) — impls + new `sheets` db (raise `MAX_DBS`).
- `src/store/postgres.rs` (modify) — impls + `sheets_connection` table + lease.
- `src/api.rs` (modify) — 4 routes + `AppState` fields.
- `src/main.rs` (modify) — build config/client, AppState wiring, scheduled-sync task.
- `web/src/routes/Extensions.tsx` (modify) + `web/src/lib/{api,queries,types}.ts` + `web/src/i18n/{en,pt-BR}.ts` + a Vitest.
- `docs/SHEETS.md` + `docs/SHEETS.PT_BR.md` (new), `docs/CONFIGURATION.md` + PT (modify), `docs/ROADMAP.md` (modify).
- `web/e2e/sheets-real.spec.ts` (new).

Reference implementations to mirror (read before writing):
- OAuth code exchange + reqwest client: `src/oidc.rs` (`exchange_code`, `OidcRuntime::init`).
- Secret masking in API responses: `src/pixel.rs` + the pixel handlers in `src/api.rs`.
- Store record round-trip + a new LMDB db + Postgres table: the pixel or webhook methods in `src/store/{mod,lmdb,postgres}.rs`.
- Lease-coordinated background task: `try_acquire_health_lease` + `spawn_link_checker` in `src/health.rs` and `src/main.rs`.
- Short code from id: `codec::to_base62(permute::encode(id, st.key))`; per-link click count: `store.visits(id)`; catalog: `store.list_links(after, limit, None, None)`.

---

### Task 1: Sheets config, connection type, and OAuth URLs (pure logic)

**Files:**
- Create: `src/sheets/mod.rs`
- Modify: `src/lib.rs` (add `pub mod sheets;`)

**Interfaces:**
- Produces:
  - `pub struct SheetsConfig { pub client_id: String, pub client_secret: String, pub redirect_url: String, pub sync_secs: Option<u64> }`
  - `pub fn SheetsConfig::from_env() -> Option<SheetsConfig>` — `Some` only when `QUARK_SHEETS_CLIENT_ID`, `_CLIENT_SECRET`, `_REDIRECT_URL` are all non-empty; `sync_secs` from `QUARK_SHEETS_SYNC_SECS` parsed as `u64`, floored to 60, else `None`.
  - `pub const SHEETS_SCOPE: &str = "https://www.googleapis.com/auth/drive.file";`
  - `pub fn connect_url(cfg: &SheetsConfig, state: &str) -> String` — Google auth endpoint `https://accounts.google.com/o/oauth2/v2/auth` with `response_type=code`, `client_id`, `redirect_uri`, `scope=SHEETS_SCOPE`, `access_type=offline`, `prompt=consent`, `state`, `include_granted_scopes=true`.
  - `pub enum SyncStatus { Never, Ok, Error(String) }` (serde tag: `{ "state": "never"|"ok"|"error", "detail"?: String }`).
  - `pub struct SheetsConnection { pub refresh_token: String, pub email: String, pub spreadsheet_id: Option<String>, pub last_sync: Option<u64>, pub last_status: SyncStatus }` (derive `Clone, Debug, Serialize, Deserialize`).

- [ ] **Step 1: Write the failing tests**

Add to `src/sheets/mod.rs` a `#[cfg(test)] mod tests`:

```rust
#[test]
fn from_env_is_none_without_all_required() {
    // Uses a helper that reads an explicit map instead of process env, so tests
    // do not touch global state. from_env delegates to from_map(&std::env::var).
    let base = |id: &str, sec: &str, red: &str| SheetsConfig::from_parts(id, sec, red, None);
    assert!(base("", "s", "r").is_none());
    assert!(base("i", "", "r").is_none());
    assert!(base("i", "s", "").is_none());
    assert!(base("i", "s", "r").is_some());
}

#[test]
fn sync_secs_floored_to_60() {
    assert_eq!(SheetsConfig::from_parts("i", "s", "r", Some(5)).unwrap().sync_secs, Some(60));
    assert_eq!(SheetsConfig::from_parts("i", "s", "r", Some(3600)).unwrap().sync_secs, Some(3600));
    assert_eq!(SheetsConfig::from_parts("i", "s", "r", None).unwrap().sync_secs, None);
}

#[test]
fn connect_url_requests_offline_consent_and_drive_file_scope() {
    let cfg = SheetsConfig::from_parts("cid", "sec", "https://h/admin/integrations/sheets/callback", None).unwrap();
    let url = connect_url(&cfg, "st4te");
    assert!(url.starts_with("https://accounts.google.com/o/oauth2/v2/auth?"));
    assert!(url.contains("client_id=cid"));
    assert!(url.contains("access_type=offline"));
    assert!(url.contains("prompt=consent"));
    assert!(url.contains("scope=https%3A%2F%2Fwww.googleapis.com%2Fauth%2Fdrive.file"));
    assert!(url.contains("state=st4te"));
    assert!(url.contains("redirect_uri=https%3A%2F%2Fh%2Fadmin%2Fintegrations%2Fsheets%2Fcallback"));
}
```

- [ ] **Step 2: Run to verify it fails**

Run: `export PATH="$HOME/.cargo/bin:$PATH" && CARGO_BUILD_JOBS=1 cargo test -j1 --lib sheets::tests 2>&1 | tail`
Expected: FAIL (module `sheets` not found / items undefined).

- [ ] **Step 3: Implement the config, types, and connect URL**

Write `src/sheets/mod.rs`. Use `serde` derives (import `serde::{Serialize, Deserialize}`), and URL-encode query params with the `form_urlencoded`/`url` crate already in the tree (check `Cargo.toml`; `url::form_urlencoded::byte_serialize` is used in `src/pixel.rs` — reuse the same approach). `from_env` reads the four env vars and delegates to `from_parts(id, secret, redirect, sync_secs)`. `from_parts` returns `None` if any of id/secret/redirect is empty. Add `pub mod sheets;` to `src/lib.rs`.

```rust
impl SheetsConfig {
    pub fn from_env() -> Option<SheetsConfig> {
        let sync = std::env::var("QUARK_SHEETS_SYNC_SECS").ok().and_then(|s| s.parse::<u64>().ok());
        Self::from_parts(
            &std::env::var("QUARK_SHEETS_CLIENT_ID").unwrap_or_default(),
            &std::env::var("QUARK_SHEETS_CLIENT_SECRET").unwrap_or_default(),
            &std::env::var("QUARK_SHEETS_REDIRECT_URL").unwrap_or_default(),
            sync,
        )
    }
    pub fn from_parts(id: &str, secret: &str, redirect: &str, sync_secs: Option<u64>) -> Option<SheetsConfig> {
        if id.is_empty() || secret.is_empty() || redirect.is_empty() { return None; }
        Some(SheetsConfig {
            client_id: id.to_string(),
            client_secret: secret.to_string(),
            redirect_url: redirect.to_string(),
            sync_secs: sync_secs.map(|s| s.max(60)),
        })
    }
}
```

`connect_url` builds the query with percent-encoding (mirror `pixel.rs`'s encoder). `SyncStatus` uses `#[serde(tag = "state", content = "detail", rename_all = "lowercase")]` — verify the shape serializes as `{"state":"ok"}` / `{"state":"error","detail":"..."}` with a quick assertion if unsure.

- [ ] **Step 4: Run to verify it passes**

Run: `CARGO_BUILD_JOBS=1 cargo test -j1 --lib sheets::tests 2>&1 | tail`
Expected: PASS (3 tests).

- [ ] **Step 5: Commit**

```bash
git add src/sheets/mod.rs src/lib.rs
git commit -m "feat(sheets): config (opt-in), connection type, OAuth connect URL"
```

---

### Task 2: The `SheetsApi` trait, code exchange, token refresh, and row building

**Files:**
- Create: `src/sheets/client.rs`
- Modify: `src/sheets/mod.rs` (add `pub mod client;`, `exchange_code`, `refresh_access_token`, `sync`, `catalog_rows`)

**Interfaces:**
- Produces:
  - In `client.rs`:
    ```rust
    #[async_trait::async_trait]
    pub trait SheetsApi: Send + Sync {
        // Creates a spreadsheet titled `title`, returns its id.
        async fn create_spreadsheet(&self, access_token: &str, title: &str) -> Result<String, String>;
        // Overwrites the values of the first sheet starting at A1 with `rows`.
        async fn update_values(&self, access_token: &str, spreadsheet_id: &str, rows: &[Vec<String>]) -> Result<(), String>;
    }
    pub struct GoogleSheetsApi { pub client: reqwest::Client }
    ```
    (`async_trait` is already a dependency — confirm in `Cargo.toml`; the `Store` trait uses it.)
  - In `mod.rs`:
    - `pub async fn exchange_code(client: &reqwest::Client, cfg: &SheetsConfig, code: &str) -> Result<TokenResponse, String>` — POST `https://oauth2.googleapis.com/token` (form: `code`, `client_id`, `client_secret`, `redirect_uri`, `grant_type=authorization_code`). `TokenResponse { access_token: String, refresh_token: Option<String>, id_token: Option<String> }`.
    - `pub async fn refresh_access_token(client: &reqwest::Client, cfg: &SheetsConfig, refresh_token: &str) -> Result<String, String>` — POST the token endpoint with `grant_type=refresh_token`, returns the access token.
    - `pub fn catalog_rows(links: &[(u64, crate::store::Record)], key: u64, base_url: &str, visits: &std::collections::HashMap<u64, u64>) -> Vec<Vec<String>>` — a header row plus one row per link: `[code, short_url, destination, created(rfc3339-ish or epoch string), clicks, tags(joined by ", "), folder]`. `code = crate::codec::to_base62(crate::permute::encode(id, key))`; `short_url = format!("{base_url}/{code}")`.
    - `pub async fn sync(store: &std::sync::Arc<dyn crate::store::Store>, api: &dyn client::SheetsApi, cfg: &SheetsConfig, key: u64, base_url: &str, conn: &mut SheetsConnection) -> Result<(), String>` — refresh token, create spreadsheet if `conn.spreadsheet_id.is_none()` (store the id into `conn`), page `list_links` to the end, gather `visits` per id, build rows, `update_values`. On success set `last_status = Ok`, `last_sync = Some(now)`. The caller persists `conn`.
- Consumes: `SheetsConfig`, `SheetsConnection` (Task 1); `crate::store::{Store, Record}`, `crate::codec`, `crate::permute`.

- [ ] **Step 1: Write the failing test for row building**

```rust
#[test]
fn catalog_rows_has_header_then_one_row_per_link() {
    use crate::store::Record;
    let rec = |url: &str, tags: Vec<&str>, folder: Option<&str>| Record {
        url: url.into(), expiry: None, created: 1_700_000_000, tags: tags.into_iter().map(String::from).collect(),
        max_visits: None, rules: vec![], variants: vec![], app_ios: None, app_android: None,
        folder: folder.map(String::from), fallback_url: None, password_hash: None,
    };
    let links = vec![(1u64, rec("https://a.com", vec!["x", "y"], Some("mkt"))), (2u64, rec("https://b.com", vec![], None))];
    let mut visits = std::collections::HashMap::new();
    visits.insert(1u64, 42u64);
    let rows = catalog_rows(&links, 0xKEY as u64, "https://s.example", &visits);
    assert_eq!(rows.len(), 3); // header + 2
    assert_eq!(rows[0], vec!["code", "short_url", "destination", "created", "clicks", "tags", "folder"]);
    assert_eq!(rows[1][2], "https://a.com");
    assert_eq!(rows[1][4], "42");
    assert_eq!(rows[1][5], "x, y");
    assert_eq!(rows[1][6], "mkt");
    assert_eq!(rows[2][4], "0"); // no visits entry -> 0
    assert!(rows[1][1].starts_with("https://s.example/"));
}
```

(Replace `0xKEY` with a literal like `0x1234`.)

- [ ] **Step 2: Run to verify it fails** — `cargo test -j1 --lib sheets::tests::catalog_rows`. Expected FAIL.

- [ ] **Step 3: Implement `catalog_rows`, the token calls, and `sync` with the trait**

Add `async-trait` if not present (it is — `Store` uses it). Implement `catalog_rows` (pure). Implement `exchange_code`/`refresh_access_token` mirroring `oidc.rs::exchange_code` (same reqwest form-POST shape, different endpoint/params, and no PKCE). Implement `GoogleSheetsApi` in `client.rs`: `create_spreadsheet` POSTs `https://sheets.googleapis.com/v4/spreadsheets` with `{"properties":{"title":title}}` and a `Bearer` header, reads `spreadsheetId` from the JSON; `update_values` PUTs `https://sheets.googleapis.com/v4/spreadsheets/{id}/values/A1?valueInputOption=RAW` with `{"values": rows}`. Implement `sync` using the trait (paginate `list_links(after, 500, None, None)` until a short page; collect `visits(id)` per link).

- [ ] **Step 4: Write and run a mock-driven `sync` test**

```rust
struct MockApi { created: std::sync::Mutex<Vec<String>>, updated: std::sync::Mutex<Vec<usize>> }
#[async_trait::async_trait]
impl client::SheetsApi for MockApi {
    async fn create_spreadsheet(&self, _t: &str, title: &str) -> Result<String, String> {
        self.created.lock().unwrap().push(title.into()); Ok("sheet123".into())
    }
    async fn update_values(&self, _t: &str, _id: &str, rows: &[Vec<String>]) -> Result<(), String> {
        self.updated.lock().unwrap().push(rows.len()); Ok(())
    }
}
```

Drive `sync` against an in-memory LMDB store (see `open_backends` usage in `tests/api_it.rs`) seeded with two links, a `SheetsConnection` with `spreadsheet_id: None`, and assert: `create_spreadsheet` called once, `conn.spreadsheet_id == Some("sheet123")`, `update_values` received `1 + link_count` rows, `conn.last_status` is `Ok`. Refresh is the one network-ish call — for this pure-logic test, `sync` should take an already-refreshed access token OR the test injects a config whose refresh is stubbed; keep `refresh_access_token` out of the `sync` seam by having `sync` accept the `api` trait only and do the refresh through a second tiny seam OR (simpler) split: `sync` takes `access_token: &str` and the caller (api handler / scheduled task) does the refresh. Prefer the split — it keeps `sync` fully mockable with no network. Update the interface: `sync(store, api, key, base_url, conn, access_token)`.

Run: `cargo test -j1 --lib sheets::tests`. Expected PASS.

- [ ] **Step 5: Commit**

```bash
git add src/sheets/
git commit -m "feat(sheets): SheetsApi seam, token exchange/refresh, row building, sync"
```

---

### Task 3: Persist the single `SheetsConnection` (Store, LMDB + Postgres) + lease

**Files:**
- Modify: `src/store/mod.rs`, `src/store/lmdb.rs`, `src/store/postgres.rs`
- Test: `tests/store_it.rs` (or the store round-trip test file used for pixels)

**Interfaces:**
- Produces on the `Store` trait:
  ```rust
  async fn put_sheets_connection(&self, c: &crate::sheets::SheetsConnection) -> Result<(), StoreError>;
  async fn get_sheets_connection(&self) -> Result<Option<crate::sheets::SheetsConnection>, StoreError>;
  async fn delete_sheets_connection(&self) -> Result<(), StoreError>;
  async fn try_acquire_sheets_lease(&self, holder: &str, ttl_secs: u64) -> Result<bool, StoreError>;
  ```
  Single record: store under a fixed key (LMDB db `sheets`, key `b"connection"`; Postgres table `sheets_connection` with a `singleton bool primary key default true` row). `try_acquire_sheets_lease` mirrors `try_acquire_health_lease` (LMDB always `true`; Postgres upsert-with-expiry).

- [ ] **Step 1: Write the failing round-trip test** (LMDB always; Postgres gated)

```rust
#[tokio::test]
async fn sheets_connection_round_trips() {
    let dir = tempfile::tempdir().unwrap();
    let (store, _sink) = open_backends(dir.path()).await.unwrap();
    assert!(store.get_sheets_connection().await.unwrap().is_none());
    let c = quark::sheets::SheetsConnection {
        refresh_token: "rt".into(), email: "me@x.com".into(),
        spreadsheet_id: Some("s1".into()), last_sync: Some(5),
        last_status: quark::sheets::SyncStatus::Ok,
    };
    store.put_sheets_connection(&c).await.unwrap();
    let got = store.get_sheets_connection().await.unwrap().unwrap();
    assert_eq!(got.email, "me@x.com");
    assert_eq!(got.spreadsheet_id.as_deref(), Some("s1"));
    store.delete_sheets_connection().await.unwrap();
    assert!(store.get_sheets_connection().await.unwrap().is_none());
}
```

- [ ] **Step 2: Run to verify it fails** — `cargo test -j1 --test store_it sheets_connection_round_trips`. Expected FAIL.

- [ ] **Step 3: Implement LMDB + Postgres**

LMDB: add a `sheets` database, raise `MAX_DBS` by 1 (find the `EnvOpenOptions::max_dbs` call), store the JSON blob under `b"connection"`. Postgres: add a `CREATE TABLE IF NOT EXISTS sheets_connection (singleton boolean primary key default true, blob jsonb not null)` in the migration/`init` path, and a `sheets_lease` mirroring the health-lease table (reuse its DDL and the `EXTRACT(EPOCH FROM now())` clock). Implement the four methods on both backends and the `StubStore` mock in `src/webhooks/delivery.rs` (returns defaults) so it still compiles. Update `reset_for_tests` to truncate the two new tables.

- [ ] **Step 4: Run to verify it passes** (both LMDB and, if available, Postgres)

```bash
CARGO_BUILD_JOBS=1 cargo test -j1 --test store_it sheets_connection_round_trips
QUARK_TEST_DATABASE_URL=postgres://quark:quark@127.0.0.1:5432/quark CARGO_BUILD_JOBS=1 cargo test -j1 --test postgres_store_it 2>&1 | tail
```
Expected: PASS (Postgres suite only if the DB is up).

- [ ] **Step 5: Commit**

```bash
git add src/store/ tests/store_it.rs src/webhooks/delivery.rs
git commit -m "feat(sheets): persist the single SheetsConnection + sync lease (LMDB + Postgres)"
```

---

### Task 4: API routes (connect / callback / sync / status) with masking + CSRF

**Files:**
- Modify: `src/api.rs`

**Interfaces:**
- Consumes: `SheetsConfig`, `SheetsConnection`, `sync`, `SheetsApi` (Tasks 1-2), the Store methods (Task 3), `admin_guard`, `csrf_guard`, `request_is_https`, the login-state signing helpers in `oidc.rs` (reuse `sign_login_state`/`verify_login_state` pattern for the `state` cookie, or a small dedicated HMAC over `state`).
- Produces new `AppState` fields: `pub sheets: Option<Arc<crate::sheets::SheetsConfig>>`, `pub sheets_api: Arc<dyn crate::sheets::client::SheetsApi>`. Routes:
  - `GET /admin/integrations/sheets/connect` (admin_guard `Full`) → 303 to `connect_url`, setting a signed short-lived `qk_sheets_state` cookie (Path=/, HttpOnly, SameSite=Lax, Max-Age=600).
  - `GET /admin/integrations/sheets/callback` (admin_guard `Full`) → verify state cookie, `exchange_code`, fetch the connected email (from the token response's `id_token` `email`, or a `userinfo` GET — prefer decoding the `id_token` payload without verification since it came straight from Google's token endpoint over TLS), build `SheetsConnection { refresh_token, email, spreadsheet_id: None, last_sync: None, last_status: Never }`, persist, clear the state cookie, 303 to the panel (Extensions).
  - `POST /admin/integrations/sheets/sync` (admin_guard `Full` + `csrf_guard`) → load connection (404 if none), `refresh_access_token`, `sync`, persist updated connection, return the status JSON.
  - `GET /admin/integrations/sheets/status` (admin_guard `Full`) → `{ connected: bool, email?, spreadsheet_url?, last_sync?, last_status }` with the refresh token NEVER included.
  - `DELETE /admin/integrations/sheets` (admin_guard `Full` + csrf via method preflight) → `delete_sheets_connection`.

- [ ] **Step 1: Write the failing test** (in `tests/api_it.rs`, LMDB, admin-token auth)

```rust
#[tokio::test]
async fn sheets_status_reports_disconnected_and_never_leaks_refresh_token() {
    // Build AppState with sheets: Some(cfg), a mock SheetsApi, admin_token set.
    // Seed a connection with refresh_token "SECRET".
    // GET /admin/integrations/sheets/status with x-admin-token
    // -> 200, body.connected == true, and the response text does NOT contain "SECRET".
}
```

Also: `POST /admin/integrations/sheets/sync` with a session cookie but no `x-quark-csrf` → 403 (reuse the session test helper from the OIDC tests).

- [ ] **Step 2: Run to verify it fails** — `cargo test -j1 --test api_it sheets_status`. Expected FAIL.

- [ ] **Step 3: Implement the routes + AppState fields + masking**

Add the two `AppState` fields and the four routes in the router builder. Mask by never serializing `refresh_token` (the status response is its own struct, not `SheetsConnection`). Reuse `csrf_guard` on `sync`. For the `state` cookie, reuse the HMAC helpers used for `qk_login` (they sign an arbitrary payload). Add `sheets: None` / a default mock or real `sheets_api` to EVERY `AppState { .. }` literal in tests/benches (script it like the `oidc_configured` rollout: insert after the `oidc` field). Confirm the count and that `--all-targets` compiles.

- [ ] **Step 4: Run to verify it passes**

```bash
CARGO_BUILD_JOBS=1 cargo build -j1 --all-targets
CARGO_BUILD_JOBS=1 cargo test -j1 --test api_it sheets
```
Expected: build clean; the sheets api tests PASS.

- [ ] **Step 5: Commit**

```bash
git add src/api.rs tests/ benches/
git commit -m "feat(sheets): connect/callback/sync/status routes (masked, CSRF-guarded)"
```

---

### Task 5: main.rs wiring + scheduled lease-coordinated sync

**Files:**
- Modify: `src/main.rs`

**Interfaces:**
- Consumes: `SheetsConfig::from_env`, `GoogleSheetsApi`, the Store lease + connection methods, `sync`.
- Produces: `AppState { sheets, sheets_api, .. }` populated; a background task spawned only when `cfg.sync_secs.is_some()`, mirroring `spawn_link_checker`: every `sync_secs`, if `try_acquire_sheets_lease(holder, ttl)` and a connection exists, refresh + `sync` + persist; log outcome (never the token).

- [ ] **Step 1: Wire AppState and log the connector state**

Mirror the OIDC block in `main.rs`: `let sheets_config = SheetsConfig::from_env(); let sheets = sheets_config.map(Arc::new);` build `let sheets_api: Arc<dyn SheetsApi> = Arc::new(GoogleSheetsApi { client: reqwest::Client::new() });` put both in `AppState`. Print `sheets sync: enabled (scheduled every Ns)` / `enabled (on demand)` / `disabled` like the other subsystems.

- [ ] **Step 2: Spawn the scheduled task when configured**

```rust
if let Some(cfg) = state.sheets.clone() {
    if let Some(secs) = cfg.sync_secs {
        let st = state.clone();
        let holder = /* same node-id/uuid pattern as the health checker */;
        tokio::spawn(async move {
            let mut tick = tokio::time::interval(std::time::Duration::from_secs(secs));
            loop {
                tick.tick().await;
                if !st.store.try_acquire_sheets_lease(&holder, secs * 2).await.unwrap_or(false) { continue; }
                if let Ok(Some(mut conn)) = st.store.get_sheets_connection().await {
                    // refresh + sync + persist; log errors, never the token
                }
            }
        });
    }
}
```

- [ ] **Step 3: Build and smoke-run**

```bash
powershell -Command "Get-Process quark -ErrorAction SilentlyContinue | Stop-Process -Force"
CARGO_BUILD_JOBS=1 cargo build -j1
# Off by default: no QUARK_SHEETS_* -> logs "sheets sync: disabled" and everything else still works.
```
Expected: builds; with no env, the connector is off and the binary runs.

- [ ] **Step 4: Commit**

```bash
git add src/main.rs
git commit -m "feat(sheets): AppState wiring + scheduled lease-coordinated sync task"
```

---

### Task 6: Frontend — the Extensions card becomes a real connector

**Files:**
- Modify: `web/src/routes/Extensions.tsx`, `web/src/lib/{api,queries,types}.ts`, `web/src/i18n/{en,pt-BR}.ts`
- Test: `web/src/routes/Extensions.test.tsx`

**Interfaces:**
- `types.ts`: `interface SheetsStatus { connected: boolean; email?: string; spreadsheet_url?: string; last_sync?: number; last_status: { state: "never" | "ok" | "error"; detail?: string } }`.
- `api.ts`: `sheetsStatus(): Promise<SheetsStatus>` (GET), `sheetsSync(): Promise<SheetsStatus>` (POST — `req` already adds `x-quark-csrf`), `sheetsDisconnect(): Promise<void>` (DELETE), and `sheetsConnectUrl(): string` returning `${BASE}/admin/integrations/sheets/connect` (full navigation, like `oidcLoginUrl`).
- `queries.ts`: a `useSheetsStatus` query + a `useSheetsSync` mutation invalidating it.

- [ ] **Step 1: Write the failing Vitest** — the Sheets card renders "Connect" when disconnected and the email + "Sync now" + last-sync status when connected; clicking "Sync now" calls `api.sheetsSync`. Mock `api.sheetsStatus`/`sheetsSync`.

- [ ] **Step 2: Run to verify it fails** — `cd web && npx vitest run Extensions`. Expected FAIL.

- [ ] **Step 3: Implement** — change the Sheets entry in `Extensions.tsx` from a static `poweredBy: "webhooks"` card to a connected-aware card driven by `useSheetsStatus`: Connect button (navigates to `sheetsConnectUrl()`) when `!connected`; when connected, show the email, a link to `spreadsheet_url`, a "Sync now" button (calls the mutation, shows a spinner and a toast), the last-sync time and error detail if any, and a "Disconnect" button. Add all new i18n keys to BOTH `en.ts` and `pt-BR.ts`.

- [ ] **Step 4: Run to verify it passes** — `npx vitest run Extensions` then the full `npx vitest run` and `npx tsc --noEmit`. Expected: PASS, no type errors (key parity holds).

- [ ] **Step 5: Commit**

```bash
git add web/src
git commit -m "feat(sheets): Extensions card connector (connect/sync/disconnect, i18n EN+PT-BR)"
```

---

### Task 7: Docs + ROADMAP

**Files:**
- Create: `docs/SHEETS.md`, `docs/SHEETS.PT_BR.md`
- Modify: `docs/CONFIGURATION.md`, `docs/CONFIGURATION.PT_BR.md`, `docs/ROADMAP.md`

- [ ] **Step 1: Write `docs/SHEETS.md` + PT twin** — what syncs (link catalog), the `drive.file` scope and why, the one-time Google Cloud setup (project, enable Sheets + Drive APIs, consent screen, Web OAuth client + redirect URI), the `QUARK_SHEETS_*` variables, on-demand vs scheduled sync, and that the refresh token is stored server-side and masked. A Mermaid diagram of the connect + sync flow. avoid-ai-writing: no em-dashes, natural PT-BR.

- [ ] **Step 2: Add the `QUARK_SHEETS_*` rows** to `docs/CONFIGURATION.md` + PT under "Admin e acesso" (mirror the OIDC row style), and move Sheets from Backlog to Done in `docs/ROADMAP.md` with a one-line description and a link to `SHEETS.md`.

- [ ] **Step 3: Commit**

```bash
git add docs/
git commit -m "docs(sheets): SHEETS.md (EN+PT), CONFIGURATION rows, ROADMAP -> Done"
```

---

### Task 8: Manual real-Google E2E checklist (skipped in CI)

**Files:**
- Create: `web/e2e/sheets-real.spec.ts`

- [ ] **Step 1: Write the skipped spec + checklist** — mirror `web/e2e/google-real.spec.ts`: `test.skip(process.env.QUARK_E2E_SHEETS !== "1", ...)`, a header comment documenting the operator steps (create OAuth client, enable APIs, set `QUARK_SHEETS_*`, run quark behind an https tunnel so Google accepts the redirect), and a light programmatic assertion that `GET /admin/integrations/sheets/status` reports `connected: true` once the operator has connected by hand. The Google consent screen blocks automation, so the connect itself is manual.

- [ ] **Step 2: Verify it is skipped by default** — `cd web && npx playwright test sheets-real`. Expected: 1 skipped.

- [ ] **Step 3: Commit**

```bash
git add web/e2e/sheets-real.spec.ts
git commit -m "test(sheets): manual real-Google E2E checklist (skipped without QUARK_E2E_SHEETS)"
```

---

## Final verification (before requesting review)

```bash
export PATH="$HOME/.cargo/bin:$PATH"
powershell -Command "Get-Process quark -ErrorAction SilentlyContinue | Stop-Process -Force"
CARGO_BUILD_JOBS=1 cargo build -j1 --all-targets
CARGO_BUILD_JOBS=1 cargo test -j1 --lib --test api_it --test store_it
CARGO_BUILD_JOBS=1 cargo fmt --check
CARGO_BUILD_JOBS=1 cargo clippy -j1 --all-targets -- -D warnings
cd web && npx tsc --noEmit && npx vitest run
```
All green. Then request the adversarial code review (scoped to `feat/sheets` in the quark repo) before merging.
