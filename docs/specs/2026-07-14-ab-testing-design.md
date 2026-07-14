# A/B testing (rotating destinations) — design + plan (roadmap #17)

**Date:** 2026-07-14 · **Branch:** feat/ab-testing (off main; no merge) · **Effort:** medium-high.

## Goal
One short code splits traffic across multiple destinations by weight (A/B/n testing). Per-variant click measurement so you can see which wins.

## Hot path
- Links WITHOUT variants (default, every existing link) pay only a `Vec::is_empty()` check — unchanged.
- Links WITH variants pay a STATELESS weighted pick per hit: one `getrandom` draw + a weighted selection over the variants. NO store write (unlike max-visits) — the choice is random, not a counter. Common path stays free.

## Decisions (locked, user delegated)
- `Record.variants: Vec<Variant>` with `#[serde(default)]` (persisted; LMDB serde + Postgres `variants JSONB NOT NULL DEFAULT '[]'` + migration + old-blob regression — the recurring lesson).
- `Variant { url: String, weight: u32 }` (weight >= 1). `pick_variant(variants, rand_u64) -> usize`: weighted selection over the sum of weights; deterministic given the random input (so it's unit-testable with a seeded value). Redirect draws the random via `getrandom` (fast, stateless).
- Redirect: if `rec.variants` non-empty → pick a variant → 302 to `variant.url`; else → 302 to `rec.url`. The chosen variant index travels on the `ClickEvent` (`variant: Option<u32>`, serde default None) for per-variant measurement.
- **SSRF: each `variant.url` passes the same validation as the main url** (is_valid_url + extract_host + is_internal_host/blocklist) at create/patch. Cap variants (e.g. 10). A link with variants still keeps its `url` as the fallback/default (used when variants is empty, or you can treat variants as the full set — keep `url` as the canonical default served when no variants).
- **Per-variant analytics**: `Aggregates.per_variant: BTreeMap<String,u64>` (keyed by variant index) with `#[serde(default)]`; `apply` increments it from `ClickEvent.variant`. ClickHouse: `variant Int32` column (default -1 = none) + migration + a per_variant GROUP BY. Stats/UI show clicks per variant.
- `create`/`patch` accept optional `variants`; LinkRow exposes them.

## Tasks
### Task 1 — backend: Record.variants + stateless weighted pick + redirect + SSRF + ClickEvent.variant + per_variant aggregate
Files: `src/store/mod.rs` (Record + Variant + pick_variant), lmdb.rs, postgres.rs (variants column + migration + all Record sites), `src/analytics/mod.rs` (ClickEvent.variant serde default + Aggregates.per_variant + apply), `src/analytics/clickhouse.rs` (variant column + migration + per_variant query), `src/api.rs` (redirect picks + sets ev.variant; create/patch accept+validate variants incl. SSRF per variant.url; LinkRow.variants), tests.
- Tests: a 2-variant link (weights 1:1) with a seeded/controlled rand splits to both; weighted (e.g. 3:1) skews correctly over many draws (statistical, with a fixed sequence or by testing pick_variant directly with boundary rand values); no-variants link unchanged; **SSRF: variant.url internal → 400/403 on create AND patch**; cap → 400; **regression: old Record without variants → []**, **old Aggregates without per_variant → {}**, **old ClickEvent without variant → None**; gated Postgres/ClickHouse round-trip. Existing redirect tests unchanged.

### Task 2 — frontend: variants editor + per-variant stats + docs
Files: CreateLinkDialog/EditLinkDialog (a "A/B variants" section: rows of {url, weight}); LinkTable (badge when a link has variants); LinkStats/StatsCharts (a per-variant clicks chart); types/i18n; `docs/AB-TESTING.md`+`.PT_BR.md`, README/ROADMAP. No em-dashes.

## Global constraints
- Common redirect (no variants) pays only `Vec::is_empty()`; the variant pick is STATELESS (getrandom, no store write); rule/variant eval reuses no store I/O.
- Every variant.url passes SSRF/blocklist validation (create AND patch).
- Persisted-struct additions (Record.variants, ClickEvent.variant, Aggregates.per_variant) → serde(default) + Postgres/ClickHouse migration + old-blob regression.
- All code English; UI i18n EN+PT; docs EN+PT_BR, no em-dashes. Rust `-j1`; gated skips clean. Stay on feat/ab-testing; no merge.

## Out of scope
- Automatic winner selection / traffic reallocation (multi-armed bandit) — a follow-up; this pass shows per-variant clicks so the operator decides.
- Sticky assignment (same visitor always gets the same variant) — needs a cookie/identifier; deferred.
