# Max-visits expiration — design + plan (roadmap #11)

**Date:** 2026-07-14 · **Branch:** feat/max-visits (off main; no merge) · **Effort:** low-medium.

## Goal
A link can expire after a maximum number of visits (in addition to, or instead of, a TTL date). A Shlink strength few OSS shorteners have.

## Key design decision (hot path)
The common redirect path must stay free. So:
- Links WITHOUT `max_visits` (the default, and every existing link) pay only a single `Option::is_none()` check on the redirect — zero store writes, zero new cost. This preserves the "redirect pays nothing" principle for the 99% case.
- Links WITH `max_visits` set pay an **opt-in cost**: on each hit, an atomic store-side increment-and-read of a per-link visit counter; when it exceeds `max_visits`, the redirect returns `410 Gone` (same as TTL expiry). Exact enforcement (no over-delivery). The operator opted into this cost, and such links (one-time/promo) are typically low-traffic.

## Decisions (locked, user delegated)
- `Record.max_visits: Option<u32>` with `#[serde(default)]` (persisted; LMDB serde + Postgres column + migration + old-blob regression — the recurring lesson).
- Visit counter stored separately from Record to avoid rewriting the whole Record per hit: `Store::bump_visits(id) -> Result<u64, StoreError>` returns the new count atomically. LMDB: a `visits` db (bump max_dbs), increment within a write txn. Postgres: `UPDATE links SET visits = visits + 1 WHERE id=$1 RETURNING visits` (atomic; `visits BIGINT NOT NULL DEFAULT 0` column + migration). A `Store::visits(id) -> u64` read for display.
- Redirect (`GET /:code`): after resolving the record and the existing TTL/expiry check, IF `rec.max_visits` is Some: `let n = store.bump_visits(id)?; if n > max_visits { return 410 }`. If None: nothing extra. Do the bump only on an otherwise-successful (found, not TTL-expired) resolution.
- `create`/`patch` accept optional `max_visits` (u32 > 0; 0/absent = unlimited). Stats/list expose `max_visits` and current `visits`.
- Cache interaction: the redirect reads the record from L1 cache as today; `max_visits` travels on the cached Record. The counter is NOT cached (must be authoritative), so the bump always hits the store — acceptable, opt-in only. Invalidate/refresh as needed on patch.

## Tasks
### Task 1 — backend: Record.max_visits + visit counter + redirect enforcement
Files: `src/store/mod.rs` (Record + `bump_visits`/`visits`), lmdb.rs, postgres.rs, `src/api.rs` (create/patch accept max_visits; redirect enforces; LinkRow exposes max_visits+visits), tests.
- Tests: create with max_visits=2 → 3rd redirect returns 410; a link without max_visits redirects unlimited (counter untouched or irrelevant); patch sets/clears max_visits; **regression: old Record blob without max_visits → None**; Postgres gated visits round-trip + atomic bump; existing redirect tests unchanged (links without max_visits behave identically).

### Task 2 — frontend + docs
Files: CreateLinkDialog/EditLinkDialog (a "max visits" optional number input alongside TTL), LinkTable (show `visits/max_visits` when set), types/i18n; `docs` mention + ROADMAP.
- The redirect-410-on-max-visits and the visits/max display. i18n EN+PT. Vitest. No em-dashes.

## Global constraints
- Common redirect (no max_visits) pays only an `is_none()` check — NO store write, NO new cost. This is the acceptance bar for the hot path.
- Record.max_visits + Postgres visits column are persisted → serde(default)/migration/old-blob regression.
- Atomic counter (LMDB txn / Postgres RETURNING) — no lost updates under concurrency.
- Exact enforcement (410 at visit max_visits+1). All code English; UI i18n EN+PT; docs EN+PT_BR, no em-dashes. Rust `-j1`; gated skips clean. Stay on feat/max-visits; no merge.

## Out of scope
- Per-visit-type limits (unique visitors); it counts raw hits.
- Resetting the counter from the UI (revisit if needed).
