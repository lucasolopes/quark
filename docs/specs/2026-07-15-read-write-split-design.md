# Postgres read/write split (multi-region reads) — design

**Branch:** `feat/read-write-split` (off `main@640747f`). NOT merged until reviewed.

**Goal:** let each region's quark read from a local Postgres read replica while all
writes go to the single primary, so a multi-region Fly.io deployment serves
redirects near the user without forking the hot path. This is the code half of
the Tier 2 target in `docs/research/2026-07-14-perf-infra.md`; the Fly artifacts
(fly.toml, primary+replica setup) follow in a separate step.

## Why a read/write split (not fly-replay)

`fly-replay` replays a whole request to another region. That is fine for a form
POST but wrong for the redirect hot path (it would send the click cross-region),
and it is Fly-specific. A read/write split is portable (any multi-region VM),
keeps the redirect entirely in-region, and matches quark's "take the dependency"
principle: lean on Postgres streaming replication instead of inventing routing.

- Writes always go to the primary (`QUARK_DATABASE_URL`), over the private
  network (Fly 6PN, no VPN).
- Reads go to a local replica (`QUARK_REPLICA_DATABASE_URL`) when set.
- The redirect never leaves the region; the existing pub/sub cache invalidation
  keeps each region's L1/L2 correct within its bounded window.

## Configuration

| Variable | Default | Meaning |
|---|---|---|
| `QUARK_DATABASE_URL` | unset (LMDB) | The Postgres **primary**: all writes, and reads when no replica is set. Unchanged. |
| `QUARK_REPLICA_DATABASE_URL` | unset | Optional local read **replica** for reads. **Unset means reads use the primary — behavior identical to today.** Only meaningful with `QUARK_DATABASE_URL` set. |

Single-region and LMDB deployments set no replica URL and are byte-for-byte
unchanged. The split is opt-in.

## Architecture

`PostgresStore` today holds one `pool: PgPool` and also implements
`AnalyticsSink`. It gains a second pool:

```rust
pub struct PostgresStore {
    write: PgPool,        // primary; was `pool`
    read: PgPool,         // replica, or a clone of `write` when no replica URL
}
```

`open(url)` stays (both pools point at `url`). A new
`open_with_replica(primary_url, replica_url)` builds `write` from the primary and
`read` from the replica. `main.rs` calls the latter when
`QUARK_REPLICA_DATABASE_URL` is set. Schema init/migration runs on `write` only
(the replica is read-only and receives the schema through replication). Each
query method picks `&self.read` or `&self.write` per the classification below;
the SQL is otherwise unchanged.

### Read vs write classification

Writes (always `write` pool): `next_id`, `put_link`, `put_alias_and_link`,
`put_link_tx`, `put_alias_and_link_tx`, `delete_link_tx`, `delete_link`,
`delete_alias`, `put_webhook`, `delete_webhook`, `next_webhook_id`,
`put_api_token`, `delete_api_token`, `next_api_token_id`, `put_session`,
`delete_session`, `gc_sessions`, `bump_visits`, `put_link_health`,
`try_acquire_health_lease`, `put_sheets_connection`, `delete_sheets_connection`,
`try_acquire_sheets_lease`, `next_pixel_id`, `put_pixel`, `delete_pixel`,
`put_wellknown`, `delete_wellknown`, `enqueue_deliveries`, `claim_due_deliveries`,
`mark_delivered`, `mark_retry`, `mark_dead`, and the `AnalyticsSink` ingest.

Reads on the **replica** (high volume, lag-tolerant): `get_link`, `get_alias`,
`list_links`, `search_links`, `list_aliases`, `list_tags`, `list_folders`,
`list_webhooks`, `get_webhook`, `list_api_tokens`, `visits`, `list_link_health`,
`link_health_for`, `list_broken_link_ids`, `list_pixels`, `get_pixel`,
`get_wellknown`, and the `AnalyticsSink` stats/aggregate reads.

Reads forced to the **primary** (read-your-writes / auth freshness, low volume,
NOT on the redirect hot path):
- `get_session_by_hash` — a user who just logged in must not be 401'd by a
  lagging replica when the panel calls `/admin/me`.
- `get_api_token_by_hash` — a freshly minted token must authenticate immediately.
- `get_sheets_connection` — read right after the OAuth callback writes it; small
  and admin-only.

Rationale: the redirect hot path (`get_link`/`get_alias` via `resolve` + cache)
is the only high-volume read and it is lag-tolerant (a brand-new link may take
the replication lag to resolve in a far region, bounded and acceptable, same
window as the cache TTL). Auth reads are low-volume and correctness-sensitive, so
they pay the primary hop.

`claim_due_deliveries` reads then writes (outbox claim) so it stays on `write`.

## Consistency notes

Replication is asynchronous (sub-second typically). Effects, all bounded by the
lag window and documented in SCALING:
- A newly created link may 404 in a distant region until the row replicates. The
  `create` response returns the computed code directly (it never reads back), so
  the API/panel is unaffected; only an immediate cross-region redirect races the
  lag.
- Analytics dashboards read the replica, so counts can trail by the lag. This
  already matches quark's at-most-once, eventually-aggregated analytics model.

## Testing

- `PostgresStore` unit/integration (gated by `QUARK_TEST_DATABASE_URL`): with no
  replica URL, `read` and `write` are the same pool and every existing test
  passes unchanged (regression).
- A new gated test builds a `PostgresStore` via `open_with_replica` pointing both
  URLs at the SAME test database (CI has no real replica) and asserts a
  write-then-read round-trips through the split routing (e.g. `put_link` then
  `get_link` returns the record), proving the wiring is correct.
- A construction test: `open(url)` yields two pools that are the same handle;
  `open_with_replica(a, b)` yields two distinct pools. Exposed via a small
  test-only accessor. A true streaming replica is exercised manually on Fly
  (documented in SCALING), since CI cannot run replication.
- `cargo test --lib`, `cargo fmt`, `cargo clippy -j1 --all-targets -- -D warnings`
  clean; `api_it` uses LMDB and is unaffected.

## Docs

`docs/SCALING.md` (+PT): a section on the read/write split, the
`QUARK_REPLICA_DATABASE_URL` variable, the consistency window, and which reads
stay on the primary and why. `docs/CONFIGURATION.md` (+PT): the new variable row.
The Fly.io multi-region guide is a separate follow-up doc.

## Global constraints

Code English; docs EN+PT-BR; the redirect hot path pays nothing new (it gains a
near-user read); no behavior change when `QUARK_REPLICA_DATABASE_URL` is unset;
LMDB backend untouched; Rust tests `-j1` / `CARGO_BUILD_JOBS=1`; Postgres tests
gated by `QUARK_TEST_DATABASE_URL`; no merge to main until reviewed.
