**English** · [Português](DEVELOPMENT.PT_BR.md)

# Development

How to build, run, and test quark locally. The backend is Rust (axum + tokio);
the admin panel is a React + Vite SPA under `web/`, built and deployed
separately from the binary.

## Prerequisites

A stable Rust toolchain via [rustup](https://rustup.rs); `rust-toolchain.toml`
pins the `stable` channel, so rustup selects it automatically. For the panel,
Node and npm. For the gated integration tests, Docker (or your own Postgres,
Valkey, and ClickHouse).

## Build and run

```bash
cargo build                 # debug build
cargo build --release       # release binary at target/release/quark

# run against a local LMDB store on the default 0.0.0.0:8080
export QUARK_KEY=$(od -An -N8 -tu8 /dev/urandom | tr -d ' ')
cargo run --release
```

With no backend variables set, quark uses the embedded LMDB store, the L1
in-process cache, and the embedded analytics sink: no external service. See
[CONFIGURATION](CONFIGURATION.md) for every variable.

The offline calibration binary that measures the Feistel diffusion and picks the
round count is separate from the service:

```bash
cargo run --bin calibrate
```

## The local full stack

`docker-compose.yml` brings up quark plus all three optional backends wired
together, matching a full multi-node deployment on one machine:

```bash
docker compose up --build
```

| Service | Image | Port |
|---|---|---|
| quark | built from the repo `Dockerfile` | 8080 |
| postgres | `postgres:16` | 5432 |
| valkey | `valkey/valkey:8` | 6379 |
| clickhouse | `clickhouse/clickhouse-server:24` | 8123 |

The compose `quark` service sets `QUARK_DATABASE_URL`, `QUARK_VALKEY_URL`,
`QUARK_CLICKHOUSE_URL`, a dev `QUARK_KEY`, a dev `QUARK_ADMIN_TOKEN`, and
`QUARK_CORS_ORIGINS` for the panel. The dev key and token are for local use
only. This stack is also the reference for running the gated integration tests.

## Tests

Unit tests live inline in `#[cfg(test)]` modules; integration tests are
`tests/*_it.rs`. The default suite needs no external service:

```bash
cargo test                                   # lib + API + unit tests
cargo fmt --all
cargo clippy --all-targets -- -D warnings    # CI enforces -D warnings
```

### Gated backend tests

The Postgres, Valkey, and ClickHouse integration tests are skipped unless the
matching URL is set. They read a separate set of variables so they never point
at a real deployment by accident:

| Variable | Gates |
|---|---|
| `QUARK_TEST_DATABASE_URL` | Postgres store, analytics, search, webhook outbox, horizontal-scale tests |
| `QUARK_TEST_VALKEY_URL` | Valkey L2 tier and pub/sub invalidation tests |
| `QUARK_TEST_CLICKHOUSE_URL` | ClickHouse sink tests |

Point them at the compose services:

```bash
export QUARK_TEST_DATABASE_URL=postgres://quark:quark@localhost:5432/quark
export QUARK_TEST_VALKEY_URL=redis://localhost:6379
export QUARK_TEST_CLICKHOUSE_URL=http://localhost:8123
```

These tests share one database and reset it between cases. Within a test binary
the `#[serial(pg)]` / `#[serial(ch)]` markers (from `serial_test`) already keep
same-backend tests from overlapping. Across binaries, cargo runs test executables
in parallel by default, so run the gated suite one binary at a time to keep two
binaries from truncating the shared database under each other:

```bash
cargo test -- --test-threads=1
# or run a single gated file
cargo test --test postgres_store_it -- --test-threads=1
```

## Web panel

```bash
cd web
npm install
npm run dev        # Vite dev server on :5173
npm run test       # Vitest
npm run build      # static build for a CDN/edge
```

Point `VITE_API_BASE_URL` at your running quark API and set
`QUARK_CORS_ORIGINS=http://localhost:5173` on the API so the browser can call
it. Auth is the same `QUARK_ADMIN_TOKEN`, entered on the panel's login screen.

## Benchmarks

Criterion benches live under `benches/`:

```bash
cargo bench --bench permute_bench     # the Feistel/ARX code generator in isolation
cargo bench --bench compare_bench     # quark vs hashids / sqids / HMAC-Feistel
cargo bench --bench redirect_bench    # the redirect hot path
```

## Where things are

The module map, backend seams, and the redirect hot path are in
[ARCHITECTURE](ARCHITECTURE.md). Deployment shapes and their limits are in
[SCALING](SCALING.md). `CONTRIBUTING.md` covers the CLA and PR expectations.
