# Dependency policy

Do not hand-roll what a well-maintained, widely-used crate already solves. Two
categories:

**Required.** `thiserror`, `tracing` + `tracing-subscriber`, `tower-http`'s
`trace` feature, `anyhow` (binaries and tests only), `secrecy` + `zeroize`. The
repo predates them; that is a gap, not a style. Adding them is part of the first
change that needs them - no separate approval, no evidence gate. See
[errors-and-observability.md](errors-and-observability.md).

**Everything else** goes through the evidence gate below. Adding a crate changes
`Cargo.toml`, the lockfile, build time and binary size, and the new code runs in
the same process that holds the database handle, the Valkey connection and the
signing keys. And nothing may cost the redirect hot path: if a candidate touches
`GET /:code`, the cache, or the store read path, `benches/redirect_bench.rs`
numbers before and after go in the PR.

## Evidence gate

Before `cargo add`, answer all eight in the PR description. An unanswered item is
a blocked dependency.

1. Downloads (total and 90-day) and reverse dependencies on crates.io / lib.rs.
2. Date of the latest release, and whether the current line is `-rc`/pre-release.
3. Crate owners: an organisation (tokio-rs, RustCrypto, rust-cli, launchbadge) or
   a single individual? What is the bus factor?
4. Open issues/PRs and response time.
5. RustSec advisories, including informational ones. Which version fixes them?
6. Declared SPDX licence - it must be compatible with **AGPL-3.0-only**. MIT,
   Apache-2.0, BSD-3-Clause, Unlicense and ISC are fine; copyleft or
   source-available licences are not.
7. Does it carry a `build.rs` or proc-macro?
8. Delta in build time and stripped binary size (`lto = true`,
   `codegen-units = 1`, `strip = true` amplify this).

Never fabricate download numbers. If you did not verify it, write "not verified".

## Hard constraints of this repo

- **No `[features]` section.** Backend selection is runtime, via env var presence.
  Do not introduce Cargo features to gate Postgres/Valkey/ClickHouse - that
  fragments a build matrix CI does not cover, and CI never passes `--features`.
- **TLS is always rustls, never OpenSSL.** `sqlx` uses `tls-rustls`, `reqwest`
  uses `default-features = false` + `rustls-tls`, `hickory-resolver` uses
  `default-features = false` + `tokio`. The runtime image is
  `debian:bookworm-slim` with no `libssl-dev`: a dependency that drags in
  native-tls breaks the container.
- `panic = "abort"` in release: no `catch_unwind`, no `#[should_panic]` validated
  in release, no relying on `Drop` during a panic.
- AGPL-3.0-only: licence compatibility is a gate, not a detail.
- Dev-only tools (`sqids`, `harsh`, `criterion`, `serial_test`) belong in
  `[dev-dependencies]` so they never enter the binary.

## What is deliberately hand-rolled - do not replace casually

These genuinely stay. The error enums, the `eprintln!` logging, the hand-rolled
access log and the bare key arrays used to be on this list; they were never
deliberate, and `thiserror`, `tracing`, `TraceLayer` and `secrecy` replaced them.

| In the repo | Instead of | Why it stays |
|---|---|---|
| `constant_time_eq` (`src/api/router.rs:3-12`) + `mac.verify_slice` | `subtle`, `constant_time_eq` crate | 10 lines, and `hmac` already compares MACs in constant time |
| `getrandom::fill` | `rand` | `src/password.rs:1-8` documents avoiding the `rand` tree just for a salt |
| `permute` Feistel + base62 `u64` ids | `uuid` | this *is* the product: short, non-sequential codes. `uuid` would work against the thesis. Do not adopt. |
| `civil_from_days` (Hinnant algorithm, `src/analytics/mod.rs:315`) | `jiff` / `chrono` / `time` | 10 tested lines, UTC only, no parsing. The trigger to adopt a date crate is weekly/monthly buckets, per-tenant timezones, or parsing date ranges from the API - then reimplementing means reimplementing tzdata, which never pays. |
| `device_from_ua` / `os_from_ua` substring heuristics | `uaparser` | the doc comment states the choice ("no external dep"); hundreds of runtime regexes conflict with redirect latency. Trigger to reconsider: the product asking for browser version or bot classification. Cheap win without a crate: lowercase once and pass `&str` to all three functions instead of allocating three Strings per event. |
| Manual validation helpers returning `Result<(), (StatusCode, &'static str)>` | `validator` | the validation that matters (`validate_webhook_url`, `validate_rules`) is async and does SSRF checks; no derive expresses that. Adopting it would leave two validation paths. |
| `RateLimiter` (fixed window, 3 modes) | `governor` | works today; migrate only if hierarchical keyed GCRA quotas are needed |
| Hand-rolled backoff + durable `next_attempt_at` | `backon` | `backon` only covers in-process retry; the durable outbox scheduler stays regardless |

## Verified candidate table

Verified 2026-07-24 against crates.io, the source repos and the RustSec
advisory database. Re-verify before acting - these numbers age.

| Crate | Latest | Last release | Maintainer | Licence | Verdict |
|---|---|---|---|---|---|
| `tracing` | 0.1.44 | 2025-12-18 | org tokio-rs | MIT | **required**; `>= 0.1.40` (RUSTSEC-2023-0078, unsound) |
| `tracing-subscriber` | 0.3.23 | 2026-03-13 | org tokio-rs | MIT | **required**; `>= 0.3.20` (RUSTSEC-2025-0055, ANSI injection) |
| `tower-http` | 0.7.0 (repo on 0.6 with `cors` + `trace`) | 2026-06-15 | org tower-rs | MIT | in use; `timeout`/`limit` still worth adding |
| `thiserror` | 2.0.19 | 2026-07-18 | dtolnay | MIT/Apache-2.0 | **required**; most-downloaded of the list, zero advisories ever |
| `anyhow` | 1.0.104 | 2026-07-18 | dtolnay | MIT/Apache-2.0 | **required, binaries and tests only**; `>= 1.0.103` (RUSTSEC-2026-0190) |
| `config` | 0.15.25 | 2026-06-26 | org rust-cli | MIT/Apache-2.0 | adopt if a config crate is ever wanted |
| `figment` | 0.10.19 | 2024-05-17 | single maintainer, stalled | MIT/Apache-2.0 | **avoid** - no release in 2+ years |
| `clap` | 4.6.4 | 2026-07-21 | org clap-rs | MIT/Apache-2.0 | adopt if the binary ever needs subcommands |
| `secrecy` | 0.10.3 | 2024-10-09 | iqlusioninc (repo active) | Apache-2.0/MIT | **required** - makes a `Debug` leak of `signing_key` impossible by construction |
| `zeroize` | 1.9.0 | 2026-06-12 | org RustCrypto | Apache-2.0/MIT | **required** - already transitive via argon2/chacha20poly1305 |
| `subtle` | 2.6.1 | 2024-06-24 | dalek-cryptography | BSD-3-Clause | not needed here (own helper + `verify_slice`) |
| `backon` | 1.6.0 | 2025-10-18 | single maintainer (Apache OpenDAL core) | Apache-2.0 | adopt if unifying retry; **the maintained replacement for the unmaintained `backoff`** |
| `tokio-util` | 0.7.19 | 2026-07-21 | org tokio-rs | MIT | not needed for shutdown here (see async-and-workers.md) |
| `jiff` | 0.2.34 | 2026-07-19 | BurntSushi | Unlicense/MIT | adopt if dates are ever needed; zero advisories ever; still 0.x |
| `time` | 0.3.54 | 2026-07-20 | jhpratt | MIT/Apache-2.0 | evaluate; pin `>= 0.3.47` (RUSTSEC-2026-0009, DoS parsing RFC 2822) |
| `chrono` | 0.4.45 | 2026-06-04 | org chronotope | MIT/Apache-2.0 | evaluate; only if a dependency forces it |
| `uuid` | 1.24.0 | 2026-07-15 | org uuid-rs | Apache-2.0/MIT | **do not adopt** - conflicts with the product's id design |
| `validator` | 0.20.0 | 2025-01-20 | single maintainer | MIT | evaluate; weakest case on the list |
| `governor` | 0.10.4 | 2025-12-16 | single maintainer | MIT | evaluate |
| `metrics` + `metrics-exporter-prometheus` | 0.24.6 / 0.18.3 | 2026-05-13 / 2026-04-30 | org metrics-rs | MIT / MIT+Apache-2.0 | evaluate - there is no `/metrics` and no way to measure redirect p99 or cache hit ratio today. Do after `tracing`, and pick one road, not both. |
| `opentelemetry` | 0.32.0 | 2026-05-08 | CNCF / open-telemetry | Apache-2.0 | evaluate later - high version churn, strict compatibility matrix |
| `rstest` | 0.26.1 | 2025-07-27 | single maintainer | MIT/Apache-2.0 | adopt if parameterized tests are wanted |
| `insta` | 1.48.0 | 2026-06-11 | mitsuhiko + max-sixty | Apache-2.0 | adopt for JSON payload snapshots |
| `testcontainers` | 0.27.3 | 2026-04-15 | org testcontainers | MIT/Apache-2.0 | adopt only with the env-var path kept as fallback; needs Docker on the runner |
| `cargo-nextest` | 0.9.140 | 2026-07-05 | nextest-rs | Apache-2.0/MIT | adopt - test groups solve the shared-Postgres race in the runner instead of via file locks |
| `cargo-deny` | 0.20.2 | 2026-07-09 | Embark Studios | MIT/Apache-2.0 | adopt - AGPL-3.0-only makes licence checking a real need, plus RustSec and duplicate detection. CI tool, not a dependency. |
| `cargo-machete` | 0.9.2 | 2026-04-15 | single maintainer | MIT | evaluate - on demand, not a required gate |
| `mimalloc` | 0.1.52 | 2026-05-22 | binding by single maintainer (mimalloc itself by Microsoft) | MIT | evaluate; `>= 0.1.39` (RUSTSEC-2022-0094). Works on Windows, unlike jemalloc. Measure with `redirect_bench` first. |
| `jemallocator` | 0.5.4 | 2023-07-27 | stalled | MIT/Apache-2.0 | **avoid** - the maintained name is `tikv-jemallocator`, which does not support windows-msvc |
| `fred` | 10.1.0 | 2025-02-27 | single maintainer | licence mismatch crates.io vs repo | **avoid** - 17 months stalled, 11x less traction than `redis-rs`, unresolved licence divergence |
| `reqwest-middleware` | 0.5.2 | 2026-05-19 | org TrueLayer | MIT/Apache-2.0 | evaluate; must match the reqwest major |
| `http-body-util` | 0.1.4 | 2026-07-13 | seanmonstar / hyperium | MIT | adopt - already transitive, declaring it is free |

## Current dependency status

Verified 2026-07-24. Useful when someone asks "are we behind?".

- **`hmac` 0.13.0 and `sha2` 0.11.0 are stable finals, not RCs** (2026-03-29 and
  2026-03-25). The exact pins in `Cargo.toml` are correct. They share `digest`
  0.11, so they move together. Their MSRV is 1.85, which is therefore the
  project's practical floor - and enough to enable edition 2024.
- **`argon2` is still 0.6.0-rc.8.** Stay on 0.5.3.
- **`axum` is on 0.8.9** (migrated in LUC-136; 0.7 had been end of line for 20
  months). Two things bit: the `/:id` to `/{id}` param syntax across every route,
  and `Option<ConnectInfo>` losing its blanket impl - see http-layer.md for the
  `MaybeConnectInfo` replacement. 0.8.2 is yanked; do not pin it.
- **`redis` 0.27 -> 1.4.1**: the crate left 0.x in Dec 2025. BSD-3-Clause (the
  driver, not the server's RSAL/SSPL - no AGPL conflict).
- **`heed` 0.20 -> 0.22.1**: two majors behind. The `read-txn-no-tls` feature the
  repo uses still exists on 0.22. Ignore the `0.22.1-nested-rtxns-N` pre-releases.
- **`sqlx` 0.8.6 -> 0.9.0**. RUSTSEC-2024-0363 (query smuggling via a truncating
  cast on payloads > 4 GiB) was fixed in 0.8.1, so the repo is covered - but the
  advisory's own recommended mitigation is limiting request body size, which
  argues for `tower-http`'s `RequestBodyLimitLayer`.
- **`url`**: keep `>= 2.5.4` and `idna >= 1.0.3` (RUSTSEC-2024-0421, punycode
  confusion). Host privilege checks in `src/api/domains.rs` and
  `src/api/sso_domains.rs` depend on this; always compare the parser-normalized
  host, never the raw URL string.
- **`moka`** enables both `sync` and `future`, compiling two cache
  implementations. Both are genuinely used (`sync` for L1/host routing, `future`
  for the per-tenant OIDC runtime), so this is intentional.
- **Duplicate crypto trees in the binary**: `sha2` 0.10.9 *and* 0.11.0, `hmac`
  0.12.1 *and* 0.13.0, `digest` 0.10.7 *and* 0.11.3, `crypto-common` 0.1.7 *and*
  0.2.2, `getrandom` 0.2.17 *and* 0.4.3, `rand` 0.8.7 *and* 0.10.2 - the direct
  deps are on the new line but transitives still pull the old one. Dead weight in
  a binary that sells itself as single-binary. `cargo-deny`'s `bans` would at
  least make it visible and intentional.

## Lint configuration: the one tooling gap worth raising

There is no `[lints]` table in `Cargo.toml`, no `clippy.toml`, no `rustfmt.toml`
and no `#![deny]` anywhere. The only barrier is CI running
`cargo clippy --all-targets -- -D warnings`, which covers the default groups only.

That matters here more than in most projects: with `panic = "abort"`, an `unwrap`
in the request path kills the process. The code already follows that discipline by
hand (zero production unwraps in `src/api/`), but the rule is written nowhere,
which makes it invisible to a new contributor or agent.

The proposal, when someone picks it up:

```toml
# Cargo.toml
[lints.rust]
unsafe_code = "deny"

[lints.clippy]
pedantic = { level = "warn", priority = -1 }   # groups need priority = -1
# anti-panic set, allowed in tests via clippy.toml
unwrap_used = "warn"
expect_used = "warn"
panic = "warn"
indexing_slicing = "warn"
string_slice = "warn"
# async safety
await_holding_lock = "warn"
let_underscore_future = "warn"
# hot-path types
result_large_err = "warn"
large_enum_variant = "warn"
```

```toml
# clippy.toml
allow-unwrap-in-tests = true
allow-expect-in-tests = true
allow-panic-in-tests = true
```

Never enable the whole `restriction` or `nursery` group - cherry-pick.
`indexing_slicing` and `string_slice` matter directly in short-code parsing
and short-code parsing. Do **not** create a `rustfmt.toml`: the project uses the
default and CI already checks it; a config file would only produce reformat churn.

New lint suppressions should use `#[expect(lint, reason = "...")]` rather than
`#[allow]`, so the warning returns when the exception stops being needed. The 11
existing `#[allow]`s (9 `clippy::too_many_arguments`, 1 `type_complexity`, 1
`dead_code`) stay as they are - not worth a sweep. Any other `allow` is a
deviation from the project's norm; fix the lint instead.
