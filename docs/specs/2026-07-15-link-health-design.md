# Broken-link monitoring (link health): design + plan

**Date:** 2026-07-15
**Branch:** `feat/link-health` (off main; no merge until reviewed)
**Effort:** medium-large. A background checker, a dedicated health store surface
(LMDB db + Postgres table), two new webhook event types, panel UI, and docs.

## Goal

Periodically check whether each link's destination still responds, record its
health, and notify (via a webhook) when a link transitions to broken or recovers.
An operator who shortened thousands of links learns a destination died from a
notification instead of a user complaint.

## Scope decisions (author's calls, "pode seguir"; documented for review)

- **Opt-in, off by default.** The checker runs only when `QUARK_HEALTH_CHECK_SECS`
  is set (seconds between sweeps, clamped to a sane minimum). Unset means no
  background HTTP is ever made, matching the project's "network backends are
  opt-in and off by default" principle.
- **One checker node.** In a multi-node deployment the sweep runs only on the node
  whose `QUARK_NODE_ID` is `0` or unset. This avoids duplicate checks without a
  distributed lease; documented as the designated-checker rule. (A Postgres lease
  is a later refinement if needed.)
- **"Broken" definition, no redirect following.** A `HEAD` (falling back to `GET`
  on 405) with a short timeout; `2xx`/`3xx` is healthy (a live server that
  redirects counts as alive), `4xx`/`5xx`/timeout/connection error is broken. Not
  following redirects sidesteps SSRF-via-redirect entirely.
- **Internal hosts are skipped.** A link whose host is `is_internal_host` is never
  probed and stays unchecked.
- **Health lives in its own store surface**, not on `Record`: a check writes
  health every sweep, so keeping it off the link record avoids rewriting (and
  cache-invalidating) the whole record on every probe.
- **Transition-only events, best-effort.** `link.broken` fires when a link goes
  healthy→broken; `link.recovered` on broken→healthy. Emitted on the in-memory
  best-effort channel like `link.clicked`/`link.expired` (not the durable outbox);
  the checker is not the hot path but health events are informational.

## Non-goals

- Distributed check leasing / sharding across nodes (designated-node rule covers
  multi-node correctness for now).
- Per-link check cadence or opt-out (global cadence; every link is checked).
- Following redirects to a final status; checking link *content* (only status).
- Retry/backoff on a single probe (one probe per sweep; a transient failure flips
  to broken and the next sweep recovers it, which the transition events reflect).

## Data model

```rust
// src/store/mod.rs
pub struct LinkHealth {
    pub checked_at: u64,       // unix seconds of the last probe
    pub status: Option<u16>,   // HTTP status, None on connection error/timeout
    pub healthy: bool,
}
```

New `Store` trait methods:
- `put_link_health(&self, id: u64, h: &LinkHealth) -> Result<(), StoreError>`
- `list_link_health(&self) -> Result<Vec<(u64, LinkHealth)>, StoreError>`, the
  checker's previous-state map and the panel's bulk read.

LMDB: a new named db `health` (`id -> JSON(LinkHealth)`), created in `open`.
Postgres: `CREATE TABLE IF NOT EXISTS link_health (id BIGINT PRIMARY KEY,
checked_at BIGINT NOT NULL, status INT, healthy BOOLEAN NOT NULL)`, upsert on id.

## Tasks

### Task 1: `LinkHealth` type + Store methods (LMDB + Postgres)

**Files:** `src/store/mod.rs` (struct + trait methods), `src/store/lmdb.rs`
(new `health` db + impls), `src/store/postgres.rs` (migration + impls), tests.

- Add `LinkHealth` and the two trait methods.
- LMDB: open a `health` db; `put_link_health` writes JSON; `list_link_health`
  iterates the db. Unit test: put then list round-trips; overwrite updates.
- Postgres: migration + upsert + select-all. Gated round-trip test.

### Task 2: `EventType::LinkBroken` / `LinkRecovered`

**Files:** `src/webhooks/mod.rs` (enum + `as_str` + `from_wire` + the chat
`channel_message` render arm), tests.

- Add both variants with wire strings `link.broken` / `link.recovered`.
- Extend `as_str`/`from_wire`/the channel message match. Unit test: round-trips
  through `as_str`/`from_wire`; a wrong string is `None`.

### Task 3: the checker worker

**Files:** `src/health.rs` (new module), `src/lib.rs`, `src/main.rs` (spawn when
configured + node is designated), tests.

- `async fn probe(client, url) -> LinkHealth`: `HEAD` then `GET` on 405; classify.
- `fn spawn_link_checker(store, webhooks, client, period, key)` -> `JoinHandle`:
  a `tokio::time::interval` loop; each tick pages `list_links`, loads the current
  `list_link_health` into a map, probes each non-internal link, writes health, and
  on a healthy↔broken transition emits `link.broken`/`link.recovered` (payload
  reuses `webhook_event_payload` with the code + url).
- Tests: `probe` classifies 200/301 healthy, 404/500 broken, unreachable broken
  (wiremock or a hyper oneshot server, as the webhook tests do). A sweep over a
  seeded store flips health and emits exactly one transition event (assert via a
  test dispatcher channel).

### Task 4: expose health in the panel API

**Files:** `src/api.rs` (`LinkRow` + `admin_links_list` join, optional
`?health=broken` filter), tests.

- `admin_links_list` reads `list_link_health` once and attaches a `health` object
  (`{healthy, status, checked_at}`) to each `LinkRow` (omitted when unchecked).
- `?health=broken` returns only links whose health is broken. Combine with
  existing paging/filters.
- Tests: a seeded broken link appears with `health.healthy=false`; the filter
  narrows to broken.

### Task 5: frontend: health badge + filter

**Files:** `web/src/lib/types.ts`, `web/src/lib/api.ts`, `LinkTable.tsx`,
`Links.tsx` (filter), `web/src/i18n/en.ts` + `pt-BR.ts`, Vitest.

- `Link.health?: { healthy: boolean; status?: number; checked_at: number }`.
- A small status dot in the table: green "OK", red "quebrado" (with the status/
  time in a title), grey "não checado" when absent. A "só quebrados" filter.
- i18n EN + PT-BR. Tests: a broken link renders the broken indicator; the filter
  requests `?health=broken`.

### Task 6: docs

**Files:** `docs/LINK-HEALTH.md` (+ PT twin, with a Mermaid sweep diagram),
`docs/CONFIGURATION.md` (+ PT) `QUARK_HEALTH_CHECK_SECS`, `docs/API.md` (+ PT)
`health` field + `?health=broken`, `docs/ROADMAP` (+ PT), README index.

## Global constraints

- No background HTTP unless `QUARK_HEALTH_CHECK_SECS` is set; the redirect hot
  path is never touched by this feature.
- The checker never probes internal hosts (`is_internal_host`) and never follows
  redirects.
- Health events are best-effort in-memory (like `link.clicked`/`link.expired`).
- Multi-node: only the `QUARK_NODE_ID` 0/unset node sweeps.
- Code in English; UI i18n EN + PT-BR; docs EN + PT_BR.
- Non-destructive Postgres migration (`CREATE TABLE IF NOT EXISTS`).
- `-j1` / `CARGO_BUILD_JOBS=1` for Rust builds/tests; kill `quark.exe` before
  building; Postgres tests gated by `QUARK_TEST_DATABASE_URL`.
