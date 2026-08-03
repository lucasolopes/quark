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
export QUARK_TEST_DATABASE_URL=postgres://quark_test:quark_test@localhost:5432/quark_test
export QUARK_TEST_VALKEY_URL=redis://localhost:6379
export QUARK_TEST_CLICKHOUSE_URL=http://localhost:8123
```

`quark_test` is not the compose stack's `quark` role, and that matters. The
compose `quark` role is the container's `POSTGRES_USER`, which Postgres creates
as a **superuser**, and Postgres exempts superusers from Row Level Security
unless the role carries `NOBYPASSRLS`. Since `FORCE ROW LEVEL SECURITY` is what
isolates tenants from each other in cloud mode, running the suite as a
superuser would test only the app-level `WHERE tenant_id` predicate and every
isolation test would pass vacuously. `docker/initdb/10-test-role.sql` creates
`quark_test` (non-superuser, `NOBYPASSRLS`, owner of its own database) on the
first `docker compose up`, and CI creates the same role. Point
`QUARK_TEST_DATABASE_URL` at a non-superuser role or
`cloud_force_rls_blocks_raw_sql_without_tenant_predicate` fails on purpose.

If the compose volume predates that init script, create the role by hand:

```bash
docker compose exec postgres psql -U quark -d quark \
  -c "CREATE ROLE quark_test LOGIN PASSWORD 'quark_test' NOSUPERUSER NOBYPASSRLS NOCREATEROLE;" \
  -c "CREATE DATABASE quark_test OWNER quark_test;"
```

These tests share one database and reset it between cases. Every case that
touches the shared backend carries `#[file_serial]` (from `serial_test` with the
`file_locks` feature), which takes a lock in a file rather than in the process.
That is deliberate: cargo runs the test executables in parallel, and a plain
`#[serial]` only serializes within one binary, so two binaries would still run
schema DDL, `FORCE ROW LEVEL SECURITY`, and `TRUNCATE` under each other.

The file lock covers the normal `cargo test` run. To rule out cross-binary
interference entirely, or when debugging a suspected race, run one binary at a
time:

```bash
cargo test -- --test-threads=1
# or run a single gated file
cargo test --test postgres_store_it -- --test-threads=1
```

`cargo test` is fail-fast: it stops at the first test binary that fails and
hides the rest, so a broken shared database shows up as one failure when it is
really dozens. Use `--no-fail-fast` (as CI does) to see the whole picture:

```bash
cargo test --no-fail-fast
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
