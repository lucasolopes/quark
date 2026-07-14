# Device-aware redirect (deep linking v1) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: superpowers:subagent-driven-development. Steps use `- [ ]`.

**Goal:** Let a link send iOS and Android clicks to platform-specific destinations (App Store / Play Store / platform web), falling back to the normal URL for desktop and unconfigured platforms.

**Architecture:** Two optional fields on `Record` (`app_ios`, `app_android`). A pure User-Agent classifier picks the destination at redirect time, only for links that set one. SSRF-validated like the main URL. Builds on the app-association hosting already on this branch.

**Tech Stack:** Rust (axum, heed/LMDB, sqlx/Postgres), React + TS.

## Global Constraints

- Code English; no inline `//` (keep `///`); UI i18n EN + PT-BR; docs EN + `.PT_BR.md`.
- SSRF guard on every destination: main `url` + `app_ios` + `app_android` (`is_blocked_target`/`is_internal_host`, http/https only).
- Hot path pays NOTHING when a link has no app destination: guard on `rec.app_ios.is_some() || rec.app_android.is_some()` before any UA work; `rec.url` still MOVED (no clone) in the common path.
- Persisted-struct lesson: new `Record` fields get `#[serde(default)]` + old-blob deserialization regression test + Postgres `ALTER TABLE ... ADD COLUMN IF NOT EXISTS` + `row_to_link` mapping.
- Deferred deep linking and in-app-browser interstitial are OUT of v1 (documented as limits).
- No merge to main. avoid-ai-writing on prose. Rust tests `CARGO_BUILD_JOBS=1 cargo test -j1`. Postgres tests gated by `QUARK_TEST_DATABASE_URL`. Kill stray `quark.exe` before a release build.

---

### Task 1: Record fields + platform classifier + resolver (Store + pure logic)

**Files:**
- Modify: `src/store/mod.rs` (`Record` struct: add `app_ios`, `app_android`; keep the pure helpers here or in api.rs per existing style)
- Modify: `src/store/postgres.rs` (migration `ADD COLUMN IF NOT EXISTS app_ios TEXT`, `app_android TEXT`; `row_to_link`; the INSERT/UPDATE of a link)
- Modify: `src/store/lmdb.rs` (no schema change; Record is a JSON blob — confirm round-trip)
- Test: inline tests in the store modules + the pure-helper module

**Interfaces:**
- Produces: `Record.app_ios: Option<String>`, `Record.app_android: Option<String>`.
- Produces (pure, in `src/api.rs` near `client_ip`): `enum Platform { Ios, Android, Other }`, `fn classify_platform(ua: Option<&str>) -> Platform`, `fn app_destination<'a>(rec: &'a Record, ua: Option<&str>) -> Option<&'a str>`.

- [ ] **Step 1: Failing store round-trip** — put a Record with `app_ios: Some("https://apps.apple.com/x")`, `app_android: None`; get it back; assert both fields survive. And an OLD-blob regression: deserialize a Record JSON WITHOUT the two fields, assert both are `None`.
- [ ] **Step 2: Run, verify fail** (`CARGO_BUILD_JOBS=1 cargo test -j1 -p quark --lib store`).
- [ ] **Step 3: Add the fields** to `Record` with `#[serde(default, skip_serializing_if = "Option::is_none")]`. Add the Postgres migration + `row_to_link` + the link INSERT/UPDATE columns. LMDB blob needs no change.
- [ ] **Step 4: `classify_platform` + `app_destination`** pure helpers with `#[cfg(test)]` unit tests: iPhone/iPad/iPod → Ios; Android → Android; desktop/empty/None → Other; `app_destination`: ios UA + app_ios set → Some(ios); android UA + only app_ios set → None; no fields → None.
- [ ] **Step 5: Run** `CARGO_BUILD_JOBS=1 cargo test -j1 -p quark --lib` green; `cargo fmt` + clippy `-D warnings`.
- [ ] **Step 6: Commit** `feat(deep-linking): per-link app destinations + platform classifier`.

### Task 2: Redirect wiring + create/patch validation (SSRF)

**Files:**
- Modify: `src/api.rs` (redirect handler: use `app_destination`; create + patch: validate `app_ios`/`app_android` like `url`)
- Test: `tests/api_it.rs`

**Interfaces:**
- Consumes: `classify_platform`/`app_destination` (Task 1); existing `is_blocked_target`, the request `user_agent` header, `RawQuery` not needed here.

- [ ] **Step 1: Failing API tests** — (a) create a link with `app_ios` set; GET with an iPhone UA → 302 Location = iOS destination. (b) same link, GET with a desktop UA → 302 Location = normal url. (c) a link with NO app fields → 302 unchanged. (d) create/patch with an internal `app_ios` (e.g. `http://127.0.0.1/`) → 400 (SSRF). (e) patch adds `app_android`; Android UA → 302 to it.
- [ ] **Step 2: Run, verify fail.**
- [ ] **Step 3: Redirect wiring** — in the redirect handler, after expiry check: `let ua = headers.get(USER_AGENT).and_then(|v| v.to_str().ok()); let dest = if rec.app_ios.is_some() || rec.app_android.is_some() { app_destination(&rec, ua).map(str::to_string) } else { None }; let location = dest.unwrap_or(rec.url);` (rec.url moved when no app dest). Use `location` in the LOCATION header.
- [ ] **Step 4: Validation** — extend the create and patch handlers: when `app_ios`/`app_android` present, parse http/https and run `is_blocked_target`; reject with 400 like the main URL. Mirror the existing url-validation code path exactly.
- [ ] **Step 5: Run** `CARGO_BUILD_JOBS=1 cargo test -j1 -p quark --test api_it` green; fmt + clippy.
- [ ] **Step 6: Commit** `feat(deep-linking): device-aware redirect + SSRF-checked app destinations`.

### Task 3: Frontend — app destinations in the link dialog

**Files:**
- Modify: the create/edit link dialog component under `web/src/` (add the section), `web/src/lib/types.ts` (Link type gains app_ios/app_android), `web/src/lib/api.ts` (create/patch payloads), `web/src/i18n/en.ts` + `pt-BR.ts`
- Test: the dialog's Vitest test file

- [ ] **Step 1: Failing Vitest** — the dialog renders an "App destinations" section with iOS and Android inputs; entering a value submits it in the create payload.
- [ ] **Step 2: Run, verify fail** (`cd web && npx vitest run`).
- [ ] **Step 3: Implement** the two optional inputs with a short note ("used only when the app is not installed and the click is from that platform"), wire into the create/patch payload and the Link type. i18n keys EN + PT-BR parity.
- [ ] **Step 4: Run** `cd web && npx vitest run && npx tsc --noEmit && npm run lint && npm run build` green.
- [ ] **Step 5: Commit** `feat(deep-linking): app destination inputs in the link dialog`.

### Task 4: Docs

**Files:**
- Modify: `docs/DEEP-LINKING.md` + `docs/DEEP-LINKING.PT_BR.md`

- [ ] **Step 1** Add a "Device-aware redirect" section: what it does, the iOS/Android/fallback behavior as a small table, that it runs only for links that set an app destination (hot path note), SSRF is applied to app destinations, and the explicit limit: deferred deep linking (app not installed) and in-app-browser routing are not handled and would need an in-app SDK. avoid-ai-writing (no em-dashes/AI-isms).
- [ ] **Step 2** Mirror to PT_BR.
- [ ] **Step 3: Commit** `docs(deep-linking): device-aware redirect section (EN+PT_BR)`.

## Self-review
- Coverage: Record+classifier (T1), redirect+SSRF (T2), panel (T3), docs (T4). Deferred/interstitial explicitly out.
- Types consistent: `app_ios`/`app_android` `Option<String>` across store/api/frontend; `classify_platform`/`app_destination` signatures fixed in T1 and consumed in T2.
- Hot path: guarded by `is_some()`; `rec.url` moved in common path.
