# Errors and observability: the required standard

This is the standard, and the repo is fully on it: every error is a `thiserror`
enum, every log line is a `tracing` event, `anyhow` covers the binary, and key
material is wrapped in `secrecy`. There is no `eprintln!` and no
`Result<_, String>` left in `src/` - if you are about to write either, you are
reintroducing something that was deliberately removed.

**Rules for any code you write or touch:**

| Concern | Required | Forbidden |
|---|---|---|
| Typed error | `#[derive(thiserror::Error)]` | hand-written `impl Display` + `impl Error` |
| Error that is only reported, never matched | `anyhow::Error` + `.context(..)`, **binaries and tests only** | `Result<_, String>` |
| Operational log | `tracing::{error,warn,info,debug}!` with structured fields | `eprintln!`, `println!` |
| Per-request log | `tower_http::trace::TraceLayer` | hand-rolled timing middleware |
| Key material / secret in a struct | `secrecy::SecretBox` / `SecretString`, or `Zeroizing<..>` | bare `[u8; 32]`, `String` |

Nothing here changes the HTTP contract: handlers still return `Response`, store
failures are still 503, error bodies are still short `&'static str` literals. This
is about how errors are *typed* and how the process *talks*, not about status
codes. See [http-layer.md](http-layer.md) for the parts that do not change.

## Enabling the crates

The dependencies are not in `Cargo.toml` yet. Adding them is part of the first
change that needs them - not a separate approval. Versions verified 2026-07-24.

```toml
[dependencies]
thiserror = "2"
tracing = "0.1.44"                      # >= 0.1.40 (RUSTSEC-2023-0078, unsound)
tracing-subscriber = { version = "0.3.23", features = ["env-filter", "json"] }
                                        # >= 0.3.20 (RUSTSEC-2025-0055, ANSI injection)
secrecy = "0.10"
zeroize = "1.9"
tower-http = { version = "0.6", features = ["cors", "trace"] }   # add "trace" to the existing entry

[dev-dependencies]
anyhow = "1.0.104"                      # >= 1.0.103 (RUSTSEC-2026-0190, unsound)
```

`anyhow` also goes in `[dependencies]` when `src/main.rs` and
`src/bin/calibrate.rs` start using it. It must **never** appear in a signature
reachable from `src/lib.rs`: consumers of the `Store` trait need to match on error
variants, and `anyhow::Error` erases them.

## Migration rule: convert the module, not the line

The one thing worse than the legacy pattern is both patterns inside one file.

- Touching a module? Convert **that module** fully: its error enum to
  `thiserror`, its `eprintln!` calls to `tracing`.
- Never add a new hand-written `Display`/`Error` impl, a new `Result<_, String>`,
  or a new `eprintln!` - not even "to match the neighbour".
- Do not sweep unrelated modules in the same commit. One module per commit keeps
  the diff reviewable and keeps `cargo test` meaningful.
- The conversions below are mechanical and behaviour-preserving. If one is not,
  that is a finding worth its own commit.

## thiserror

```rust
#[non_exhaustive]
#[derive(Debug, thiserror::Error)]
pub enum StoreError {
    #[error("database error")]
    Db(#[from] heed::Error),
    #[error("serialization error")]
    Serde(#[from] serde_json::Error),
    /// Driver errors are flattened to a string on purpose: it keeps sqlx/redis/
    /// clickhouse out of `quark::store`'s semver surface and keeps the `Err`
    /// small on the redirect hot path.
    #[error("storage backend failure: {0}")]
    Backend(String),
    #[error("unique constraint violated")]
    UniqueViolation,
    #[error("operation not supported by this backend")]
    Unsupported,
}
```

- **`#[non_exhaustive]` on every public error enum.** New backends add variants
  regularly; without it each one is a breaking change.
- **`#[from]` only for local, unambiguous 1-to-1 causes** (heed, serde_json).
  `#[from]` generates `impl From<Dep::Error>`, which puts that dependency in the
  public contract - so a driver error from sqlx/redis/clickhouse keeps going
  through the `backend<E: Display>(e)` constructor into `Backend(String)`. That
  flattening was already the right call; `thiserror` does not change it.
- **Never lose the cause.** `#[from]` implies `#[source]`; use explicit
  `#[source]` when the variant needs its own context alongside the underlying
  error. `.map_err(|_| MyError::X)` throws away the only diagnostic you will have
  in production - carry it whenever the variant can hold it. The exception stays
  `src/secretbox.rs`, where discarding detail is deliberate (a decrypt failure
  must not explain *why*).
- **Message style:** lowercase, no trailing punctuation, no `Error:` prefix, and
  do not repeat the cause in a variant that already has a `#[source]` - whoever
  reports it prints the whole chain, so repeating produces
  `"failed to write: failed to write: broken pipe"`.
- Keep the enums small and per-module. Do not merge them into one `AppError` with
  40 variants; the point of the type is that a caller can act on the variant.
  `StoreError::UniqueViolation` vs `Backend` exists precisely so a handler can
  answer 409 vs 503.

The ten enums to convert, all mechanically: `StoreError`
(`src/store/mod.rs:262-306`), `TierError` (`src/cache/mod.rs:22`), `PixelError`
(`src/pixel.rs:66`), `DnsError` (`src/dns.rs:16`), `SecretBoxError`
(`src/secretbox.rs:64`), `SignError` (`src/webhooks/mod.rs:170`, which has two
separate impls), `ParseError` (`src/import.rs:57`), `VerifyError`
(`src/oidc.rs:324`), `CreateError` (`src/api/links.rs:294`), `KcError`
(`src/keycloak/mod.rs:17`).

### Killing `Result<_, String>`

~20 signatures use it: `src/oidc.rs` (discover, fetch_jwks, select_key, init,
from_config, exchange_code, verify), `src/sheets/client.rs`, `src/sheets/mod.rs`,
`src/slack.rs:97`, `src/cluster.rs:12`, `src/health.rs:183`,
`src/api/links_admin.rs:687`. Each one costs the caller the ability to tell "IdP
unreachable" from "unknown kid" - both arrive as a string it can only log.

Replace with a module-level enum. Where the string currently lands in a
user-visible field (`reason` / `detail` in a JSON summary), keep that behaviour by
formatting the enum at the boundary, not by keeping the error a `String`.

One nuance worth stating: `StoreError` is used as an umbrella beyond its scope -
the `AnalyticsSink` trait returns `Result<(), StoreError>` even when the sink is
ClickHouse and no store is involved (`src/analytics/mod.rs:330`). Give
`AnalyticsSink` its own error type when you convert that module.

## anyhow

Binaries and tests only: `src/main.rs`, `src/bin/calibrate.rs`, `tests/**`.

```rust
#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let store = open_backends(&data_dir, multi_tenant).await
        .context("opening storage backends")?;
    Ok(())
}
```

This replaces the `eprintln!("FATAL: {msg}"); std::process::exit(1);` pattern
(`src/main.rs:51-61`) and the boot `.expect(..)` calls: returning `Err` from
`main` prints the full context chain and exits non-zero. Note that with
`panic = "abort"` there is no panic backtrace; the error backtrace via
`RUST_BACKTRACE` still works.

`std::process::exit` outside boot remains forbidden, and a handler must still
never panic.

## tracing

Every `eprintln!` becomes a `tracing` event whose fields are real fields, not a
JSON string built by hand.

```rust
// before
eprintln!("{}", serde_json::json!({"webhook_delivery_error": e.to_string(), "url": &sub.url}));
// after
tracing::error!(error = %e, url = %sub.url, "webhook delivery failed");
```

Conversion table for the forms in the repo:

| Legacy | Becomes |
|---|---|
| `eprintln!("{}", json!({"x_error": e.to_string()}))` | `tracing::error!(error = %e, "x failed")` |
| `eprintln!("WARNING: ...")` | `tracing::warn!(...)` |
| `eprintln!("<feature>: enabled (...)")` boot line | `tracing::info!(...)` |
| `eprintln!("FATAL: {msg}"); exit(1)` | return `Err(anyhow!(..))` from `main` |
| `println!("{}", access_log_line(..))` | `TraceLayer` (below) |

- Field syntax: `%value` for `Display`, `?value` for `Debug`, bare `field = value`
  for primitives. The message is a short human phrase; the data goes in fields.
- Choose the level deliberately: `error!` for something that needs attention,
  `warn!` for degraded-but-continuing (every fail-open path), `info!` for boot and
  lifecycle, `debug!`/`trace!` for detail that must be off in production. The
  point of adopting this is being able to change verbosity without recompiling.
- Event names stay English and stable - they are what someone greps or alerts on.
- Instrument long-lived tasks with a span so a webhook delivery error can be tied
  back to the request that produced it: `#[tracing::instrument(skip(st))]` on the
  function, or an explicit span in each worker loop.
- **Never log a secret.** See [security.md](security.md) - this is exactly why
  `secrecy` is required at the same time.

Subscriber init, first thing in `main`:

```rust
use tracing_subscriber::{fmt, EnvFilter};

let filter = EnvFilter::try_from_default_env()          // RUST_LOG
    .unwrap_or_else(|_| EnvFilter::new("info,quark=info"));
let fmt_layer = fmt::layer().with_target(true);
// JSON in production, human-readable locally
if std::env::var("QUARK_LOG_FORMAT").as_deref() == Ok("json") {
    tracing_subscriber::registry().with(filter).with(fmt_layer.json()).init();
} else {
    tracing_subscriber::registry().with(filter).with(fmt_layer).init();
}
```

`RUST_LOG` is the ecosystem standard and `EnvFilter` reads it by default - use it
rather than inventing `QUARK_LOG_LEVEL`. `QUARK_LOG_FORMAT` follows the project's
prefix rule and, like every env var, needs a row in `docs/CONFIGURATION.md` and
its `.PT_BR.md` twin.

Free benefit: `tokio`, `sqlx`, `reqwest`, `hickory-resolver` and `moka` already
emit `tracing` internally. Today all of that telemetry is discarded.

### Access log

`tower-http` is already a dependency; add the `trace` feature and delete the
hand-rolled middleware (`access_log_line` + `log_requests`,
`src/api/router.rs:19-44`) and the `QUARK_ACCESS_LOG` gate, which `RUST_LOG`
subsumes.

```rust
let app = app.layer(tower_http::trace::TraceLayer::new_for_http());
```

The stdout/stderr split disappears with it: the subscriber owns the destination.
Verify the Fly/Coolify log pipeline still captures what you expect after the
switch.

## secrecy and zeroize

Key material must not be a type that can be printed.

```rust
use secrecy::{ExposeSecret, SecretBox};

pub struct AppState {
    /// HMAC key for unlock cookies and OAuth login state. `SecretBox` so a
    /// stray `{:?}` cannot print it.
    pub signing_key: SecretBox<[u8; 32]>,
}

let mac = Hmac::<Sha256>::new_from_slice(st.signing_key.expose_secret())?;
```

- Apply to `AppState::signing_key` (`src/api/mod.rs:41`) and to every key in
  `src/secretbox.rs`: `from_key`, `from_keys`, the `Vec<[u8; 32]>` of old keys,
  and the `decode_key` return.
- `SecretBox`'s `Debug` prints `[REDACTED]` and its `Drop` zeroes the memory;
  `expose_secret()` then marks textually every place that touches key material,
  which is exactly what an audit of a rotating envelope-encryption module wants to
  be able to grep.
- Use `Zeroizing<Vec<u8>>` for intermediate buffers, such as the base64 decode of
  an env var.
- `zeroize` is already in the tree transitively via `argon2` and
  `chacha20poly1305`, so the marginal cost is near zero.
- **`#[derive(Debug)]` on a type containing a secret stays forbidden** regardless.
  `secrecy` makes the mistake survivable, not acceptable. And with
  `panic = "abort"` destructors do not run on abort, so zeroing is hygiene for the
  normal path only - not a guarantee.

## What does not change

- Handlers return `Response`; no `impl IntoResponse` for an error type, no `?` in
  a handler. `thiserror` types live below the HTTP layer; the handler still
  matches variants and picks a status.
- Error response bodies stay short `&'static str` literals. Never put a
  `thiserror` `Display` string from a driver into a response body - the cause goes
  to `tracing`, the client gets a stable literal.
- Fail-open behaviour stays fail-open. Converting `eprintln!` to `tracing::warn!`
  must not turn a degraded path into a propagated error.
- The named-const discipline for timeouts, capacities and limits stays.
