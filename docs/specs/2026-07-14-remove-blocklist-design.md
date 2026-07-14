# Remove the operator domain blocklist — design

**Branch:** `fix/remove-blocklist` (off `main@ac5e943`). NOT merged.
**Why:** the owner does not see value in the operator-managed destination-domain blocklist. Remove that feature end to end. KEEP the SSRF guard (`is_internal_host` / internal-and-self checks) — that is security, not the blocklist.

## Critical distinction (do NOT remove the SSRF guard)
`is_blocked_target` in `src/api.rs` currently does BOTH: (a) `is_internal_host(host)` + a self-host check (SSRF / anti-loop, SECURITY, STAYS), and (b) `st.blocklist.is_blocked(host)` (the operator blocklist, REMOVE). After this change `is_blocked_target` keeps only (a). `src/abuse/mod.rs::is_internal_host`, `extract_host`, and all their tests STAY unchanged. Every destination (link url, rules, variants, app_ios/android, webhook url) keeps its internal-host SSRF check.

## Remove, end to end

### Backend
- Delete `src/abuse/blocklist.rs` (the `Blocklist` struct, snapshot, TTL, Valkey L2, `invalidate_local`). Remove its `mod blocklist;` in `src/abuse/mod.rs` and any `host_in_blocklist` helper that exists only for it (keep `is_internal_host`/`extract_host`).
- `src/api.rs`: remove the `/admin/blocklist` routes and their handlers (`blocklist_get`/`blocklist_add`/`blocklist_delete`), the `AppState.blocklist` field, and the `st.blocklist.is_blocked(...)` branch inside `is_blocked_target` (keep the internal-host + self-host branches). Remove the `admin_guard(&st, &headers, Scope::Blocklist)` calls (they were only on the blocklist endpoints).
- `src/auth.rs`: remove the `Scope::Blocklist` enum variant, its wire string, and its unit-test lines. Renumber nothing else; the other scopes stay.
- `src/store/mod.rs` + `lmdb.rs` + `postgres.rs`: remove the trait methods `add_blocked_domain`/`remove_blocked_domain`/`list_blocked_domains` and their impls; drop the LMDB `blocked` database (lower `MAX_DBS` by 1) and the Postgres `blocked_domains` CREATE TABLE (leave no reference). Existing deployments keep an orphaned unused table/db, which is harmless (do NOT emit a destructive DROP). Update `reset_for_tests` to stop truncating the removed table.
- `src/main.rs`: remove the `Blocklist::new` wiring, the `blocklist` in `AppState`, the `QUARK_BLOCKLIST_TTL` env and `DEFAULT_BLOCKLIST_TTL_SECS`.
- `src/invalidate.rs`: remove the `blocklist` invalidation path entirely — the `Invalidation::Blocklist` variant, its parse (`"blocklist"`), the `publish("blocklist")` call (there is no more blocklist to publish for; the only publisher was blocklist add/remove), and the subscriber dispatch to `state.blocklist.invalidate_local()`. Keep the `link:<id>` cache invalidation intact (that is the valuable half). Update the module doc comment.
- `src/webhooks/delivery.rs`: remove the three blocked-domain methods from the `StubStore` mock.
- `src/cluster.rs`: fix the doc comment that mentions "cache and blocklist invalidation" -> just "cache invalidation".

### Frontend (`web/`)
- Delete `web/src/routes/Blocklist.tsx` and `web/src/routes/Blocklist.test.tsx`.
- Remove the Blocklist nav entry (`Shell.tsx`) and route (`router.tsx`).
- Remove the `Blocklist` scope option from the token scopes UI (`CreateTokenDialog.tsx`) and any Blocklist scope label in `Tokens.tsx`.
- Remove blocklist functions/types from `web/src/lib/api.ts`, `queries.ts`, `types.ts`; check `mutation-error.ts` for a blocklist reference and clean it.
- Remove all `blocklist.*` i18n keys from `web/src/i18n/en.ts` and `pt-BR.ts` (keep parity).

### Tests
- Remove the blocklist integration tests (the `/admin/blocklist` tests in `tests/api_it.rs`, any dedicated blocklist test), the blocklist unit tests, and the pub/sub `blocklist` test cases in `src/invalidate.rs` (keep the `link:<id>` tests). Keep the `is_internal_host` SSRF tests. Keep the create-to-internal-destination-403 tests (SSRF, still valid). Remove/adjust any test that asserted create-to-blocklisted-domain-403 (that behavior is gone).

### Docs
- Remove blocklist from `README`/`README.PT_BR` (docs list + feature list), `docs/ARCHITECTURE`/PT_BR (module table + any redirect/create-flow mention), `docs/API`/PT_BR (the `/admin/blocklist` endpoints + the Blocklist scope), `docs/CONFIGURATION`/PT_BR (`QUARK_BLOCKLIST_TTL`), `docs/SCALING`/PT_BR (the blocklist staleness/invalidation rows — keep the cache-invalidation ones), `docs/API-TOKENS`/PT_BR (the Blocklist scope), and `docs/ROADMAP` if it lists it. Delete any dedicated blocklist doc if one exists. avoid-ai-writing on any edited prose (no em-dashes/AI-isms), pt-BR natural.

## Behavior after
- Creating a link to any PUBLIC destination succeeds. Creating a link to an INTERNAL/loopback/self destination is still 403 (SSRF guard intact). There is no `/admin/blocklist`, no Blocklist token scope, no `QUARK_BLOCKLIST_TTL`. The panel has no Blocklist page. Cross-node cache invalidation (`link:<id>`) still works; there is just no blocklist channel message.

## Testing
`CARGO_BUILD_JOBS=1 cargo test -j1 -p quark --lib --test api_it` green; the gated Postgres/Valkey tests still green (`QUARK_TEST_DATABASE_URL`, `QUARK_TEST_VALKEY_URL`); `cargo fmt` + `cargo clippy -j1 --all-targets -- -D warnings` clean; frontend `cd web && npx tsc --noEmit && npx vitest run && npm run build` green. Confirm the SSRF create-to-internal-403 test still passes.

## Global constraints
Code English; no inline `//`; the SSRF guard and hot path untouched; single-node LMDB behavior unchanged apart from the removed blocklist; docs EN+PT_BR; no merge to main; Rust tests `-j1`; gated tests still pass.
