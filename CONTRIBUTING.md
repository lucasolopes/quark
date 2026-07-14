**English** · [Português](CONTRIBUTING.PT_BR.md)

# Contributing to quark

Thanks for your interest! quark is open source under the **GNU AGPLv3** (see
[`LICENSE`](LICENSE)). Contributions of code, docs, tests, and bug reports are
welcome.

## Contributor License Agreement (required)

Before your pull request can be merged, you must accept the
[Contributor License Agreement](CLA.md). It is a **license grant, not a copyright
transfer** — **you keep full ownership of your contributions**. You grant the
maintainer a broad license (including the right to relicense) so quark can be
offered both under the AGPL and, separately, under a commercial license and a
hosted edition. This is the same model used by peers in the space (Dub, n8n,
Grafana).

Signing is a **one-time click**: when you open your first PR, an automated bot
posts a link; accept it once and it covers all your future PRs.

## Development

Prerequisites: a stable Rust toolchain (via [`rustup`](https://rustup.rs)).

```bash
cargo build
cargo test          # lib + API tests — no external services needed
cargo fmt --all
cargo clippy --all-targets -- -D warnings
```

Integration tests for Postgres / Valkey / ClickHouse are gated behind env vars
(`QUARK_TEST_DATABASE_URL`, `QUARK_TEST_VALKEY_URL`, `QUARK_TEST_CLICKHOUSE_URL`)
and are skipped when unset — you don't need those services for most changes.

## Before you open a PR

- `cargo fmt --all` and `cargo clippy --all-targets -- -D warnings` must be clean
  (CI enforces `-D warnings`).
- Add or update tests for any behavior change. Keep the **redirect hot path**
  allocation-light — it's the performance-critical path (see
  [`benches/redirect_bench.rs`](benches/redirect_bench.rs)).
- Keep changes focused; explain the what and the why in the PR description.
- For larger changes, open an issue first to align on direction.

## Where things are

- [`docs/ARCHITECTURE.md`](docs/ARCHITECTURE.md) — how the pieces fit together.
- [`docs/ROADMAP.md`](docs/ROADMAP.md) — direction and what's next.
- [`docs/SCALING.md`](docs/SCALING.md) — deployment shapes and their limits.
