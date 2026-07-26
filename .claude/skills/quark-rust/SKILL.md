---
name: quark-rust
description: Use when writing, reviewing, or debugging Rust in this repo (src/**, tests/**, benches/**, Cargo.toml) - adding an endpoint, handler, Store method, worker, migration, env var, constant, or crypto/tenancy code; when a change needs cargo fmt/clippy/test to pass; or when deciding whether to add a crate.
---

# Rust in quark

## Overview

quark is a single-binary URL shortener: axum 0.8 + tokio, pluggable backends
(LMDB embedded by default, opt-in Postgres / Valkey / ClickHouse), 29k lines of
Rust in `src/`. It has strong, unusual, and *deliberate* conventions that a
generic "idiomatic Rust" reflex will break.

**Core principle: match the repo's conventions where they are deliberate, and the
required standard where the repo is behind.** This skill records both, with
`file:line` evidence, and never lets "that is how it is today" stand in for a
justification.

Two categories, and you must not confuse them:

- **Deliberate decisions** - the HTTP shape, the status contract, the named-const
  discipline, `getrandom` over `rand`, `permute` over `uuid`, fail-open at the
  async edges, the tenancy model. Match these.
- **Legacy being replaced** - hand-written error enums, `Result<_, String>`,
  `eprintln!` logging, bare `[u8; 32]` keys. New and touched code uses
  `thiserror`, `tracing`, `anyhow` (binaries only) and `secrecy` instead. See
  [references/errors-and-observability.md](references/errors-and-observability.md).

One constraint governs every conversion: **nothing may slow the redirect hot
path.** Where a fix touches `GET /:code`, the cache, or the store read path,
measure with `benches/redirect_bench.rs` before and after, and say so in the PR.

## The nine hard rules

Violate one of these and the change is wrong even if it compiles and tests pass.

1. **No `unwrap()` outside `#[cfg(test)]`.** `[profile.release] panic = "abort"`
   (`Cargo.toml:69-73`): a panic in the request path kills the process, it does
   not return 500. `src/api/*.rs` has zero production unwraps. `expect()` only at
   boot or on a documented structural invariant, with a message stating the
   invariant.
2. **Handlers return `Response`, never `Result`.** No `?` in a handler, and there
   is not a single `impl IntoResponse` for an error type in the crate. See
   [references/http-layer.md](references/http-layer.md).
3. **Store errors are 503, not 500.** 107 `SERVICE_UNAVAILABLE` vs 4
   `INTERNAL_SERVER_ERROR` in `src/api/`. External service failure is 502.
   Disabled surface is 404, never 403.
4. **No magic values.** Every timeout, TTL, cap, capacity, header name and cookie
   name is a `const` at the top of the owning module with a `///` explaining
   *why that number*. 125 consts in `src/`.
5. **`std::env::var` only at boot.** `main.rs` or an `X::from_env()` called from
   boot. Handlers read `AppState` fields. Prefix is always `QUARK_`
   (`QUARK_TEST_` for tests).
6. **`TenantId` is the first argument of every tenant-owned Store method**, and
   it comes from `admin_guard`'s `Principal.tenant` - never from the body, query,
   or a hardcoded `DEFAULT_TENANT`.
7. **Every field added to a persisted type gets `#[serde(default)]`** plus a test
   deserializing the old JSON literal. LMDB stores serialized JSON; without it,
   existing links stop redirecting.
8. **Every outbound IO call has a named-const timeout**, and network tiers
   fail open (log, fall back) instead of propagating.
9. **The verification gate below runs and passes before you claim done.**

## Verification gate

On this machine cargo is **not on PATH**. Use the full path.

```sh
~/.cargo/bin/cargo.exe fmt --all
~/.cargo/bin/cargo.exe fmt --check
~/.cargo/bin/cargo.exe clippy --all-targets -- -D warnings
~/.cargo/bin/cargo.exe test
```

- `--all-targets` is not optional: it is what covers `tests/` (30 targets),
  `benches/` (3 criterion targets) and `src/bin/calibrate.rs`. CI runs exactly
  this (`.github/workflows/ci.yml:53-63`).
- Never pass `--features` / `--all-features`: `Cargo.toml` has **no `[features]`
  section**. Backend selection is runtime, by env var presence.
- `cargo test` with no env vars must pass: backend tests self-skip.
- Touching `web/`? The gate is four npm scripts run with cwd `web/`:
  `npm run lint` (oxlint, `--max-warnings 0`), `npm run typecheck` (`tsc -b`),
  `npm run test`, `npm run build`.
- `cargo bench` and `npm run e2e` are **not** in CI. Run them yourself when you
  touch what they cover.
- Push to `main` auto-deploys the backend to Fly. `deploy-backend` needs only the
  `check` job, so a broken frontend still deploys the backend - verify `web/`
  locally.

## Quick reference

| Decision | The quark way | Not |
|---|---|---|
| Handler signature | `pub(crate) async fn f(State(st): State<Arc<AppState>>, ..., body) -> Response` | `-> Result<Json<T>, E>`, `impl IntoResponse` |
| Authorization | `match admin_guard(&st, &headers, Scope::X).await { Ok(p) => p, Err(s) => return s.into_response() }` inside the handler | middleware layer, custom extractor |
| Store failure | `StatusCode::SERVICE_UNAVAILABLE` (503) | 500 |
| Write that can collide | `conflict_or_503(e).into_response()` | bare 503 |
| Third-party HTTP failure | 502 `BAD_GATEWAY` | 503 |
| Backend cannot do it | `Err(StoreError::Unsupported)` -> 501 | `unimplemented!()` |
| Error body | `(StatusCode::BAD_REQUEST, "invalid host")` - short, lowercase, `&'static str` | `Json({"error": ...})`, `e.to_string()` |
| Error type | `#[derive(thiserror::Error)]` + `#[non_exhaustive]` | hand-written `Display`/`Error`, `Result<_, String>` |
| Opaque error (binaries, tests) | `anyhow::Error` + `.context(..)` | `Result<_, String>`, `eprintln!` + `exit(1)` |
| Runtime log | `tracing::error!(error = %e, url = %u, "delivery failed")` | `eprintln!`, `println!` |
| Access log | `tower_http::trace::TraceLayer` | hand-rolled timing middleware |
| Key material in a struct | `secrecy::SecretBox<[u8; 32]>` | bare `[u8; 32]`, `String` |
| Route param | `"/admin/links/{code}"` (axum 0.8) | `"/:code"` (that was 0.7) |
| Backend injection | `Arc<dyn Store>` in `AppState` | generics on handler/state |
| Hot-path event | `try_send` | `send().await` |
| Short lock | `std::sync::Mutex`, guard dropped before `await` | `tokio::sync::Mutex` |
| CPU-bound (argon2) | `tokio::task::spawn_blocking` | direct call in handler |
| Randomness | `getrandom::fill(&mut buf)` | `rand` crate |
| Secret comparison | `constant_time_eq` (`src/api/router.rs:3-12`) or `mac.verify_slice` | `==` |
| Test AppState | `common::TestState::new(store, sink)...build()` | `AppState { .. }` literal |
| API test request | `router(state)` + `tower::ServiceExt::oneshot` | real TCP server |
| Gated test | `let Some(x) = fresh().await else { eprintln!("skip: ..."); return; };` | `#[ignore]`, `unwrap()`, panic |

## Reference files

Read the one that matches what you are touching. Do not read all of them.

- [references/errors-and-observability.md](references/errors-and-observability.md)
  - **read this before writing any error type or log line.** `thiserror`,
  `anyhow`, `tracing`, `TraceLayer`, `secrecy`: what to use, how to enable it, and
  how to convert a module without leaving two styles in one file.
- [references/http-layer.md](references/http-layer.md) - handlers, status
  contract, `admin_guard`, `csrf_guard`, scopes, router, request/response types,
  PATCH semantics.
- [references/config-and-constants.md](references/config-and-constants.md) - env
  vars, `from_env`/`from_parts`, constant naming, boot logging, the
  documentation duty.
- [references/store-and-backends.md](references/store-and-backends.md) - the
  `Store`/`AnalyticsSink`/`CacheTier` traits, serde schema evolution, Postgres
  `init_schema` migrations, LMDB sub-dbs, RLS and tenant isolation.
- [references/async-and-workers.md](references/async-and-workers.md) - worker
  shape, channels, snapshots, locks, timeouts, backoff, leases, shutdown.
- [references/testing.md](references/testing.md) - `TestState`, `oneshot`,
  gating, `#[file_serial]`, fakes, virtual time, benches.
- [references/security.md](references/security.md) - crypto, tokens, secretbox,
  SSRF, rate limit, cookies, OIDC, secret redaction.
- [references/dependencies.md](references/dependencies.md) - when a crate may be
  added, the evidence gate, and a verified table of candidates with verdicts.
- [references/recipes.md](references/recipes.md) - end-to-end checklists: new
  admin endpoint (8 files), new persisted field, new env var, new worker, new
  Store method, new bench.

## Common mistakes

- **Copying the legacy pattern because the neighbouring line uses it.** A new
  `eprintln!`, a new hand-written `impl Display for MyError`, or a new
  `Result<_, String>` is wrong even surrounded by twenty of them. Convert the
  module you touch; do not extend the debt.
- **Sweeping the whole repo in one commit.** One module per commit, and a module
  is never left half-converted.
- **Adding a dependency to solve something the repo already solved.** There is a
  `constant_time_eq` helper, a `getrandom`-based token generator, a `permute`
  code generator, a `TestState` builder. `thiserror`/`tracing`/`anyhow`/`secrecy`
  are required; anything else goes through the evidence gate in
  [references/dependencies.md](references/dependencies.md).
- **Slowing the redirect path to improve something else.** Per-request spans,
  extra allocations, a blocking-pool hop on a point read: measure with
  `benches/redirect_bench.rs` or do not ship it.
- **Writing `"/:code"` in a route.** That was axum 0.7. The repo is on 0.8, where
  the param syntax is `"/{code}"` and the old form panics at startup.
- **Running `cargo clippy` without `--all-targets`** and then claiming green.
- **Adding an env var without touching `docs/CONFIGURATION.md` and its
  `.PT_BR.md` twin.** That page declares itself complete.
- **Building `AppState` by struct literal in a test.** ~26 fields and growing.
- **Reading `std::env::var` in a handler or a worker loop.**
- **Prose comments or log keys in Portuguese.** Code, comments and log keys are
  English; Portuguese lives in `docs/*.PT_BR.md` and in the panel's i18n.
