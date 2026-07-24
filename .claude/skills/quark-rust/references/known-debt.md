# Known debt and internal inconsistencies

Where the repo does the same thing two ways, this is which side to copy. Do not
"harmonize" these as a side effect of another task; each one is its own change.

All of this is tracked work, not permanent state. The constraint on every item:
**nothing may slow the redirect hot path** (`GET /:code`, the cache, the store
read path). Where a fix touches it, `benches/redirect_bench.rs` numbers before
and after go in the PR.

Epic **LUC-124**:

| Issue | Item |
|---|---|
| LUC-125 | SSRF: unify the internal-IP classification (real bypass via IPv4-compatible IPv6). Urgent. |
| LUC-126 | `secrecy` + `zeroize` for key material |
| LUC-127 | `[lints]` + `clippy.toml` |
| LUC-128 | graceful shutdown |
| LUC-129 | `thiserror`, and the end of `Result<_, String>` |
| LUC-130 | `tracing` + `TraceLayer` |
| LUC-131 | `anyhow` in the binaries; `env::var(SECRET).unwrap_or_default()` |
| LUC-132 | `connect_timeout` on every reqwest client |
| LUC-133 | test-suite hygiene |
| LUC-134 | docs debt and the remaining magic values |
| LUC-135 | CI: `deploy-backend` does not wait for the `web` job; no `cargo-deny` |
| LUC-136 | dependency updates (axum 0.7 is end of line) |

Already tracked elsewhere: **LUC-103** covers the silent analytics drop.

Deliberately **out of scope** until a benchmark justifies them: moving LMDB point
reads to `spawn_blocking`, swapping the global allocator, and a per-request span
on the redirect path.

## Being replaced (do not extend)

| Legacy in the repo | Required instead |
|---|---|
| 10 hand-written error enums with manual `Display` + `Error` | `#[derive(thiserror::Error)]` + `#[non_exhaustive]` |
| ~20 `Result<_, String>` signatures | a module-level typed enum |
| 119 `eprintln!` / 123 `println!` (56 with hand-built `serde_json::json!`) | `tracing` with structured fields |
| `access_log_line` + `log_requests` + `QUARK_ACCESS_LOG` | `tower_http::trace::TraceLayer` + `RUST_LOG` |
| `eprintln!("FATAL: ..") ; exit(1)` at boot | `anyhow::Result` from `main` with `.context(..)` |
| bare `[u8; 32]` keys in `AppState` and `secretbox.rs` | `secrecy::SecretBox` (+ `Zeroizing` for buffers) |
| `StoreError` without `source()` / `#[non_exhaustive]` | both, via `thiserror` |
| `StoreError` used as the `AnalyticsSink` error | its own error type |

Full rules and the module-at-a-time conversion procedure:
[errors-and-observability.md](errors-and-observability.md).

## Two ways to do the same thing

| Topic | Dominant / correct | Outlier - do not copy |
|---|---|---|
| `async_trait` on a data-plane trait | `#[async_trait::async_trait]` full path inline (12 sites) | `use async_trait::async_trait;` + `#[async_trait]` (9 sites, the newer HTTP/DNS seams) |
| Trait bound for a pluggable backend | `Send + Sync + 'static` (Store, AnalyticsSink, CacheTier) | `Send + Sync` only (Dns, KeycloakAdmin, SheetsApi) |
| Tenant scoping in production | explicit `st.store.m(p.tenant, ..)` (136 sites) | `ScopedStore` / `for_tenant` (tests only, 0 uses in `src/api`) |
| Module error type | see "Being replaced" above - `thiserror` enum | both legacy forms: hand-written impls (4 of 5 seams) and `Result<_, String>` (`SheetsApi` + ~20 signatures in oidc/sheets/slack/cluster/health) |
| CSRF check | `csrf_guard(&headers)` (4 sites) | manual `headers.get(HEADER_CSRF).is_none()` in `oidc_logout` |
| OAuth `state` comparison | `constant_time_eq` (sheets, slack) | `!=` in the OIDC callback (`oidc_login.rs:169-172`) - the most sensitive of the three |
| Boolean env var | `.map(\|v\| v != "0")` (3 sites, and what `CONFIGURATION.md` documents) | `matches!(.., Ok("true") \| Ok("1"))` (`keycloak/mod.rs:87-90`) |
| Runtime log line | `tracing` with fields - see "Being replaced" above | both legacy forms: `eprintln!` + `json!` (56 of 119) and prose interpolation (`main.rs`, `invalidate.rs`) |
| RNG panic message | `"system RNG must be available"` (7 sites) | `"system randomness source unavailable"` (`auth.rs:74`, `secretbox.rs:182`) |
| Periodic task | named `spawn_*` in the owning module (5 workers) | inline `tokio::spawn` block in `main.rs` (sheets sync ~70 lines, session GC, retention purge) |
| Channel-close handling | analytics: final flush, then `break` | webhook worker: `None => break` with no drain |
| Snapshot timeout const | reuse the module's existing const | the same 3s value exists twice: `PIXEL_SNAPSHOT_TIMEOUT` and `SNAPSHOT_TIMEOUT` |
| Test webhook dispatcher | `common::test_webhook_dispatcher()` (or omit the setter) | a local copy - 12 files still have one, 0 use the shared helper |
| Gated-test skip | `else { eprintln!("skip: ..."); return; }` (~155) | silent `else { return; }` (26), and one `.unwrap()` in `postgres_analytics_it.rs:311` |
| Integration test filename | `<area>_it.rs` (27 files) | `tenant_isolation.rs`, `tenant_enforcement.rs`, `store_trait.rs` |
| Reqwest client construction | named builder fn + timeout const | 4 sites with an inline literal timeout (`main.rs:249`, `oidc.rs:617`, `keycloak/client.rs:20`, `sheets/client.rs:34`) |
| Suppression attribute | `#[expect(lint, reason = "...")]` for new ones | the 11 existing `#[allow]`s with no reason - leave them |

## Real gaps

- **No graceful shutdown.** `axum::serve` has no `with_graceful_shutdown`, no
  `select!` has a cancellation arm, and `tokio::signal` is unused even though the
  `signal` feature is enabled. On SIGTERM the analytics buffer (up to 10k events)
  and the webhook queue are lost, and `panic = "abort"` means no destructors run.
  The analytics drain path is only exercised in tests. Fix it wholesale or not at
  all; no new dependency is needed.
- **Silent event drop.** `src/api/links.rs:1333` does
  `let _ = st.analytics_tx.try_send(ev);` with no log or counter, so a slow
  consumer erases clicks and conversions without a trace. The correct shape is
  `try_emit` in `src/webhooks/delivery.rs:91-100`. For the hot path, count drops
  in an `AtomicU64` and log an aggregate on the 5s tick rather than one line per
  drop.
- **15 `map_err(|_| ..)`** discard the cause. Deliberate in `secretbox.rs` (a
  decrypt failure must not explain why); worth carrying the cause in `import.rs`
  / `pixel.rs` when the variant can hold it. Fixed as part of the `thiserror`
  conversion of those modules.
- **Two divergent internal-IP classifiers.** `abuse::is_internal_host` (dominant,
  9 call sites) misses `is_documentation()` ranges and IPv4-compatible IPv6, which
  `health::is_internal_ip` (1 call site) covers. `[::127.0.0.1]` is blocked by the
  health checker but passes link creation.
- **No `connect_timeout`** on any reqwest client: a destination that accepts TCP
  and stalls in the TLS handshake consumes the whole budget.
- **LMDB blocking calls inside `async fn`** without `spawn_blocking`, including
  `write_txn` + `commit` (which fsyncs). Fine for a point read; a scan or a large
  commit is not.
- **Real sleeps in tests**: `tests/pixel_forward_it.rs:375` (5.5s of wall clock),
  `src/webhooks/delivery.rs:2006` (300ms).
- **Three files touch shared Postgres without `#[file_serial]`**:
  `tenant_isolation.rs`, `tenant_enforcement.rs`, `pubsub_invalidation_it.rs`.
- **No `[lints]`, `clippy.toml` or `rustfmt.toml`**; the anti-panic discipline is
  followed by hand but written nowhere. See dependencies.md for the proposal.
- **`deploy-backend` only `needs: check`**, not `[check, web]`, so a push to
  `main` with a broken frontend still deploys the backend to Fly.
- **Four env vars are undocumented**: `QUARK_SLACK_CLIENT_ID`,
  `QUARK_SLACK_CLIENT_SECRET`, `QUARK_SLACK_REDIRECT_URL`,
  `QUARK_KEYCLOAK_PANEL_URL`. (`QUARK_SCHEMA_LOCK_ID` is a Rust const in
  `src/store/postgres.rs:103`, not an env var, despite the name.)
- **Magic values that survived**: `3600` twice in `main.rs` (session GC at :617,
  retention purge at :655), the 5s analytics flush timer at
  `analytics/mod.rs:619`, and the sheets lease cap `secs.min(300)` at
  `main.rs:531-536`.
- **`env::var(SECRET).unwrap_or_default()`** turns missing config into an empty
  string in `src/oidc.rs:56-58` and `src/keycloak/mod.rs:82-143`.
- **Portuguese comments** in `src/analytics/mod.rs:610-616`, the only ones in
  `src/`.
- **`docs/DEVELOPMENT.md:96-98`** still documents `#[serial(pg)]` / `#[serial(ch)]`
  while the code has 188 `#[file_serial]` and zero `#[serial]`. Follow the code.
- **`web/src/lib/variants.ts:9-10`** mirrors `MAX_VARIANTS` with a stale comment
  pointing at `src/api.rs`, which is now `src/api/links.rs`.
- **`web/.npmrc` declares `save-exact=true`** but `package.json` is dominated by
  `^` ranges (~45 vs 4 exact). Declared and actual disagree; pick one explicitly
  rather than ignoring it.
- **`web/tsconfig.app.json` does not set `"strict": true`.** Handle `null` /
  `undefined` explicitly; do not assume the compiler catches it. Turning strict on
  is a broad change, not a PR-sized tweak.
- **`benches/redirect_bench.rs:81-113`** is the last `AppState` struct literal in
  the repo and already divergent (`real_ip_header` hardcoded to
  `"cf-connecting-ip"`).
