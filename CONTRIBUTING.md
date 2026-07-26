**English** · [Português](CONTRIBUTING.PT_BR.md)

# Contributing to quark

Thanks for your interest. quark is open source under the **GNU AGPLv3** (see
[`LICENSE`](LICENSE)). Contributions of code, docs, tests, and bug reports are
welcome.

By taking part you agree to the [Code of Conduct](CODE_OF_CONDUCT.md).

## Contributor License Agreement (required)

Before your pull request can be merged, you must accept the
[Contributor License Agreement](CLA.md). It is a **license grant, not a copyright
transfer**: **you keep full ownership of your contributions**. You grant the
maintainer a broad license (including the right to relicense) so quark can be
offered both under the AGPL and, separately, under a commercial license and a
hosted edition. Same model as Dub, n8n and Grafana.

Signing is a **one-time click**: when you open your first PR a bot posts a link;
accept it once and it covers every future PR.

## Ways to contribute

- **Questions and setup help** go to
  [Discussions, Q&A](https://github.com/lucasolopes/quark/discussions/categories/q-a),
  not the issue tracker.
- **Bugs** go to the [bug form](https://github.com/lucasolopes/quark/issues/new?template=bug.yml),
  with a reproduction against a fresh instance.
- **Security problems** never go in public. See [SECURITY.md](SECURITY.md).
- **Picking up work**: issues labeled `good first issue` and `help wanted` are
  free to take. Comment on the issue first so two people do not write the same
  patch.

## Development

Prerequisites: a stable Rust toolchain (via [rustup](https://rustup.rs)) and
Node 20+ for the admin panel. Depth lives in
[docs/DEVELOPMENT.md](docs/DEVELOPMENT.md); this is only what you need for a
first PR.

Backend:

```bash
cargo build
cargo test          # lib + API tests, no external services needed
cargo fmt --all
cargo clippy --all-targets -- -D warnings
```

Admin panel (`web/`, React + TypeScript + Vite):

```bash
cd web
npm ci
npm run dev
npm run lint        # oxlint, --max-warnings 0
npm run typecheck   # tsc -b, this one has broken a deploy before
npm run test        # Vitest
npm run build
```

Integration tests for Postgres, Valkey and ClickHouse are gated behind
`QUARK_TEST_DATABASE_URL`, `QUARK_TEST_VALKEY_URL` and
`QUARK_TEST_CLICKHOUSE_URL`, and are skipped when unset. Most changes do not
need them.

## Tests

- API surface: integration tests in `tests/*_it.rs`. Build the `AppState`
  through the shared `TestState` builder in `tests/common/mod.rs`, not a
  hand-rolled struct literal.
- Units: inline `#[cfg(test)]` modules next to the code.
- Panel: `web/src/**/*.test.tsx` with Vitest.
- Keep the **redirect hot path** allocation-light. It is the performance
  critical path, see [`benches/redirect_bench.rs`](benches/redirect_bench.rs).

## Docs and i18n rules

Two rules that are easy to miss and that we will ask for in review:

1. **Every user-facing doc has a `.PT_BR.md` twin.** `docs/WEBHOOKS.md` and
   `docs/WEBHOOKS.PT_BR.md`. Both start with the language switch header:
   `**English** · [Português](X.PT_BR.md)` and the mirror on the twin. A doc PR
   in one language only is incomplete.
2. **Every new panel string goes in both `web/src/i18n/en.ts` and
   `web/src/i18n/pt-BR.ts`.** No hardcoded strings in components.

Prose style: plain direct technical English, no em dashes, natural pt-BR on the
twin. Do not translate literally, write it as the language would.

## New dependencies

"Zero runtime dependencies" and "~1 MB binary" are the project's pitch. A new
crate needs a justification in the PR description: what it does, why the std
library or an existing dependency cannot, and what it costs in binary size. For
`web/` the bar is higher still, since the bundle ships to every panel user.

## Commits, branches, and pull requests

- Branches: `feat/short-slug`, `fix/short-slug`, `chore/short-slug`.
- Commits: [Conventional Commits](https://www.conventionalcommits.org/) with a
  scope, `feat(web):`, `fix(api):`, `docs:`, `chore:`. **Write commit messages
  in English.** Older history is in Portuguese; it stays as is.
- Update `CHANGELOG.md` **and** `CHANGELOG.PT_BR.md` under `## [Unreleased]` in
  the same PR, not later. Same bilingual-twin rule as every other user-facing
  doc. Your entry is what ships in the release notes, so write it for a reader
  who was not in the pull request.
- Fork the repo and open the PR against `main`.

Merging does not release anything. A git tag is the only thing that publishes an
image and deploys, so your change sits on `main` until the next version is cut.
That is why the `## [Unreleased]` entry matters: it is what the release is
assembled from.

What the merge gate looks like, so nothing surprises you:

- CI must pass: `cargo fmt --check`, `cargo clippy -D warnings`, `cargo test`,
  cargo-deny, dependency review, and the web `lint`/`typecheck`/`test`/`build`
  job.
- One approving review is required, and every review thread must be resolved.
- **Any push after an approval dismisses that approval.** The branch rule is
  "require last push approval", so a last-minute typo fix means asking for
  review again. Batch your changes.
- Expect a first response within a week. Silence means "not looked at yet".

## What we will not merge

Saving you the work up front:

- changes to the redirect hot path that trade latency for convenience
- a new store backend without someone committed to maintaining it
- a dependency on an external service in the default path
- mass style rewrites, reformatting, or renames unrelated to a fix
- changes to the short code scheme. It is the core of the project and needs a
  design spec in `docs/specs/` agreed before any code.

## Who decides

quark has a single maintainer, @lucasolopes, who has the final say on scope,
design, and what gets merged. The direction is public in
[docs/ROADMAP.md](docs/ROADMAP.md), and design specs land in `docs/specs/`
before the code does, so you can argue with a decision while it is still cheap
to change.

The [CLA](CLA.md) lets the project be offered under the AGPL and, separately,
under a commercial license. That is a deliberate choice, not a step toward
closing the source: the AGPL edition is the project, not a teaser. If a
governance model with more than one maintainer ever makes sense, it will be
written down here first.

## Where things are

- [`docs/ARCHITECTURE.md`](docs/ARCHITECTURE.md): how the pieces fit together.
- [`docs/DEVELOPMENT.md`](docs/DEVELOPMENT.md): the full development guide.
- [`docs/ROADMAP.md`](docs/ROADMAP.md): direction and what is next.
- [`docs/SCALING.md`](docs/SCALING.md): deployment shapes and their limits.
