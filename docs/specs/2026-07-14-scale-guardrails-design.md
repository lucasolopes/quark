# Scale guardrails + node-id + LMDB single-node docs — design (scale-audit #5)

**Branch:** `feat/scale-hardening` (stacked). NOT merged.
**Closes:** scale-audit gaps #5 (rate-limit N× in memory mode with no enforcement), #6 (LMDB is single-node for shared data; node_id uniqueness not validated), and the "document the real scale limits" recommendation.

## Problem

quark can be misconfigured into a broken "multi-node" deployment that looks fine but is not:
- **LMDB + `QUARK_NODE_ID`**: the id space is partitioned so ids do not collide, but each replica has its OWN LMDB file. Replica B cannot read links created by replica A. Today `main.rs` just logs `LMDB node-id: {n}`, which reads like an endorsement of multi-node when the store is not shared.
- **Multi-node without Valkey**: rate-limit is per-node (real limit becomes N×), and cache/blocklist coordination depends on the #1 pub/sub which needs Valkey. There is no guardrail; the operator only gets a `per replica (memory)` log line that is easy to miss.
- **`QUARK_NODE_ID` uniqueness** cannot be validated in-process (no coordination), and a duplicate silently collides ids.

## Design (guardrails, not silent behavior changes)

1. **LMDB + node_id loud warning.** In `main.rs`, when `QUARK_NODE_ID` is set AND the backend is LMDB (no `QUARK_DATABASE_URL`), replace the endorsing log with a prominent multi-line WARNING: LMDB stores are per-node, replicas do NOT share links, and true multi-node requires the Postgres backend. Still a warning (not a hard exit) so existing single-file setups that harmlessly set a node id keep working.

2. **`QUARK_STRICT_CLUSTER=1` fail-fast.** A new opt-in env. When set, quark refuses to start (clear error + non-zero exit) unless BOTH `QUARK_DATABASE_URL` (shared store) AND `QUARK_VALKEY_URL` (shared rate-limit + cross-node invalidation) are present. This gives operators running a real cluster a hard guardrail that the shared-state dependencies are actually wired, instead of discovering N× rate limits and stale caches in production. Single-node (flag unset) is completely unaffected.

3. **node_id uniqueness is documented, not faked.** The parse already fails on an invalid id; add a one-line note in the log and the docs that the id MUST be unique per replica (e.g. a StatefulSet ordinal) and that quark cannot detect a duplicate.

4. **Docs — the honest scale matrix.** Update `docs/SCALING.md` (+ `.PT_BR.md`) with:
   - Single-node (default): LMDB + in-memory cache/rate-limit is correct and needs no dependencies.
   - Multi-node: requires Postgres (shared store) + Valkey (shared rate-limit, cross-node cache/blocklist invalidation via the #1 pub/sub channel) + ClickHouse recommended for analytics (append-only, scalable; Postgres analytics is correct but a per-link hotspot).
   - What `QUARK_STRICT_CLUSTER` enforces.
   - node_id: LMDB-only, must be unique per replica, 8-bit node / 32-bit counter → 256 nodes max, ~4.29B links per node; Postgres uses a shared sequence with a global 2^40-link ceiling (the permute width).
   - The bounded staleness windows and how #1's pub/sub closes them (prompt in the common case, per-node TTL as the backstop).

## Files
- `src/main.rs` — the LMDB+node_id warning, the `QUARK_STRICT_CLUSTER` fail-fast check (early, before binding), and the node_id-uniqueness note.
- `docs/SCALING.md` + `docs/SCALING.PT_BR.md` — the matrix above. `docs/DEPLOY.md`/PT_BR — a pointer + the strict-cluster env var.
- If there is a config/env reference doc, add `QUARK_STRICT_CLUSTER`.

## Testing
- The startup logic is in `main()` which is not unit-tested today; extract the small decision into a pure helper if practical (e.g. `fn cluster_preflight(strict: bool, has_pg: bool, has_valkey: bool) -> Result<(), String>`) and unit-test it: strict + both present → Ok; strict + missing either → Err with a clear message; non-strict → always Ok. Keep `main()` calling the helper and exiting on Err.
- Confirm single-node (no envs) still starts and behaves identically.

## Global constraints
Code English; no inline `//`; single-node behavior unchanged; guardrails are opt-in (strict) or warnings (never a silent behavior change); docs EN + PT_BR, avoid-ai-writing (no em-dashes/AI-isms), Mermaid where a diagram helps the scale matrix; no merge to main; Rust tests `-j1`.
