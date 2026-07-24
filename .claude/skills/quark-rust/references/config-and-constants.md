# Config, env vars and constants

## No magic values

125 consts live in `src/`. Every tunable is one of them, declared at the top of
the module that owns the behaviour, with a `///` explaining **why that number**,
not what it is.

```rust
/// Per-request timeout for outbound webhook POSTs. Kept short because the
/// dispatcher shares the runtime with the redirect hot path.
pub const DELIVERY_TIMEOUT_SECS: u64 = 5;
```

Rules:

- `SCREAMING_SNAKE_CASE`, unit suffix `_SECS` / `_MS` / `_BYTES`, prefix
  `MAX_` / `MIN_` / `DEFAULT_` for limits. Or a `Duration` const directly when
  the value is always used as one.
- `pub const` when `main.rs` or another module needs it (`L1_TTL_SECS`,
  `DELIVERY_TIMEOUT_SECS`, `WEBHOOK_CHANNEL_CAPACITY`, `MIN_CHECK_SECS`);
  `pub(crate) const` for API limits inside `src/api/`; private `const` otherwise.
- Long durations are readable arithmetic, not opaque numbers: `12 * 3600`,
  `7 * 24 * 3600`, `365 * 86_400`. Large capacities use separators: `100_000`.
- Process-wide capacities live in `main.rs:15-18` (`CACHE_CAPACITY`,
  `ANALYTICS_CHANNEL_CAPACITY`); everything else lives in the owning module and
  `main.rs` imports it (`src/main.rs:8-11`).
- Header and cookie names are consts, never repeated literals:
  `HEADER_ADMIN_TOKEN` `"x-admin-token"`, `HEADER_CSRF` `"x-quark-csrf"`,
  `SESSION_COOKIE` `"qk_session"`, `LOGIN_COOKIE` `"qk_login"`. Panel cookies use
  the `qk_` prefix.
- Query param clamps use the const: `.clamp(1, MAX_PAGE_LIMIT)`, never a literal.

API limits that already exist in `src/api/` - reuse instead of inventing:
`MAX_RULES`, `MAX_VARIANTS`, `DEFAULT_PAGE_LIMIT`, `MAX_PAGE_LIMIT`, `MAX_BULK`,
`PIXELS_CAP`, `MAX_API_TOKENS`, `MAX_WEBHOOK_SUBSCRIPTIONS`, `WELLKNOWN_MAX`,
`SESSION_TTL_SECS`, `INVITE_TTL_SECS`, `MIN_ALERT_WINDOW_SECS`.

If a limit is also enforced in the panel, mirror it as a TS const with a comment
naming the Rust source, the way `web/src/lib/variants.ts:9-10` mirrors
`MAX_VARIANTS` from `src/api/links.rs:220`. There is no code generation; silent
divergence makes the UI accept what the API rejects with 400.

## Env vars

Prefix is always `QUARK_` (`QUARK_TEST_` for test-only vars). Read with
fully-qualified `std::env::var("QUARK_X")` - there is no config crate, no
`dotenv`, no central `Config` struct, and no `build.rs`. ~68 reads across 10
files.

**Read only at boot**, in `main.rs` or in an `X::from_env()` that boot calls.
Store the typed, normalized value in an `AppState` field with a doc comment
naming the source var. Handlers read `st.field`.

The tolerated exceptions, and their reasons, are: `store::open_backends`
(backend selection), `LmdbStore::open` (`QUARK_NODE_ID`, with
`open_with_node_id` so tests avoid the global race), and `api::router()` (runs
once while building the Router). `src/api/tenants.rs:85` reads
`QUARK_OIDC_REDIRECT_URL` inside a handler - that is the single real violation.
Do not copy it.

### Parsing

```rust
// numeric: tolerant, never unwrap
let per_min: u32 = std::env::var("QUARK_RATELIMIT_PER_MIN")
    .ok().and_then(|s| s.parse().ok()).unwrap_or(0);

// string with default
let data = std::env::var("QUARK_DATA").unwrap_or_else(|_| "./data".into());

// optional
let host = std::env::var("QUARK_PUBLIC_HOST").ok().filter(|s| !s.is_empty());

// boolean: dominant form is `v != "0"` (documented in CONFIGURATION.md as
// "any value other than 0"); `.is_ok()` when mere presence means on
let multi = std::env::var("QUARK_MULTI_TENANT").map(|v| v != "0").unwrap_or(false);
```

Never `unwrap()`/`expect()` on the *value* of an env var. The one deliberate
fail-fast is `parse_node_id`, which returns a `StoreError` explaining the
expected range (`src/store/lmdb.rs:155-170`).

`std::env::var(SECRET).unwrap_or_default()` is an anti-pattern present in
`src/oidc.rs:56-58` and `src/keycloak/mod.rs:82-143`: it turns missing config
into a silent empty string. For a new required value, validate and exit like
`cluster_preflight` does (`src/main.rs:51-61`).

Hosts that will be compared are normalized at the entry point:
`.trim().trim_end_matches('.').to_ascii_lowercase()` (`src/main.rs:284-289`).
The normalization exists so a mixed-case env value cannot bypass the self-loop
and domain-claim checks.

### Opt-in feature config

Each optional feature owns a config struct in its own module with
`from_env() -> Option<Self>`, returning `None` (feature off) when the trigger
var is absent or empty. Secondary fields use `.unwrap_or_default()`. The struct
lands in `AppState` as `Option<Arc<XConfig>>`.

Triggers: `QUARK_OIDC_ISSUER`, `QUARK_KEYCLOAK_BASE_URL`,
`QUARK_ENCRYPTION_KEY`, or the full OAuth triple (client_id + secret +
redirect_url) for Sheets and Slack. Evidence: `src/oidc.rs:49-79`,
`src/sheets/mod.rs:37-60`, `src/slack.rs:33-50`, `src/keycloak/mod.rs:136-151`,
`src/secretbox.rs:113-139`.

### `from_env` / `from_parts`

When config has validation or normalization, split it:

```rust
pub fn from_env() -> Option<Self> {         // only reads the process env
    Self::from_parts(std::env::var("QUARK_SHEETS_CLIENT_ID").ok()?, /* ... */)
}
/// Used by `from_env` and by tests, so tests do not need to mutate process env.
pub fn from_parts(id: String, secret: String, redirect: String, sync: u64) -> Option<Self> { /* ... */ }
```

Tests call `from_parts` (or `LmdbStore::open_with_node_id`) and **never**
`set_var` - the global env is shared across concurrent tests.

### Non-trivial default logic goes in a pure function

When resolving a value has a real rule (default per mode, fallback on bad parse,
list splitting), extract a pure function taking `Option<&str>` / `Option<String>`,
keep it log-free so it is unit-testable, and let `main.rs` do the warning log.
Examples: `retention_secs_from` (`src/main.rs:32-47`, tested at `:688-720`),
`parse_cors_origins` (`src/api/router.rs:46-56`), `parse_node_id`,
`normalize_admin_host` (`src/api/router.rs:64`).

### Boot logging

Every opt-in feature logs one line at boot naming the var that enables it:

```rust
match secs {
    Some(s) => tracing::info!(interval_secs = s, "link checker enabled"),
    None => tracing::info!("link checker disabled (set QUARK_HEALTH_CHECK_SECS to enable)"),
}
```

- Missing-but-important config: `tracing::warn!` explaining the consequence, then
  continue (fail open). A security feature disabled by absence always warns.
- Config that makes the service unserviceable: return
  `Err(anyhow::anyhow!(..))` from `main` with `.context(..)`, which prints the
  chain and exits non-zero. The legacy form
  `eprintln!("FATAL: {msg}"); std::process::exit(1);` (only `cluster_preflight`
  does it today) is being retired. Never `process::exit` outside startup, never
  panic in a handler.

## Backend selection

One decision point, `store::open_backends` (`src/store/mod.rs:1118-1168`):

```rust
match std::env::var("QUARK_DATABASE_URL") {
    Ok(url) => /* Postgres */,
    Err(_)  => /* embedded LMDB */,
}
```

`QUARK_CLICKHOUSE_URL` overrides only the `AnalyticsSink` (the Store doubles as
the embedded sink). `QUARK_VALKEY_URL` enables the L2 tier + global rate limit +
cross-node pub/sub invalidation. `main.rs` re-reads the var only to log which
backend came up. L2 connection failure degrades with a WARNING (fail open);
Store failure propagates.

Do **not** add Cargo `[features]` for this. There is no `[features]` section and
CI never passes `--features`.

## Documentation duty

A new env var is not done until it has a row in **both**
`docs/CONFIGURATION.md` and `docs/CONFIGURATION.PT_BR.md`, in the right section
(Essentials / Backends / Cluster / Auth and CORS / Cloud / Rate limit and SSRF),
with a "Default" column describing behaviour when absent. That page states it
documents every variable.

Currently undocumented (known debt, close it if you touch those files):
`QUARK_SLACK_CLIENT_ID`, `QUARK_SLACK_CLIENT_SECRET`,
`QUARK_SLACK_REDIRECT_URL`, `QUARK_KEYCLOAK_PANEL_URL`, `QUARK_SCHEMA_LOCK_ID`.

## Frontend env

The panel has exactly one env var: `import.meta.env.VITE_API_BASE_URL`, cast as
`string | undefined`, always with the trailing slash stripped
(`.replace(/\/+$/, "")`). Two fallback conventions coexist by purpose: `?? ""`
for a relative same-origin path (`web/src/lib/api.ts:22`), and
`|| window.location.origin` when building an absolute URL shown to the user
(`short-url.ts:16`, `Shell.tsx:139`, `LinkTable.tsx:43`).
