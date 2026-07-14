# Geo/device segmented redirects — design + plan (roadmap #12)

**Date:** 2026-07-14 · **Branch:** feat/geo-redirects (off main; no merge) · **Effort:** medium-high.

## Goal
One short code resolves to different destinations by rule (visitor country / device), with a default. A differentiator (Shlink-style rule engine), a natural fit for the computed-code model.

## Scope decision
- Rules match on **country** (from the `cf-ipcountry` header, already captured at redirect) and **device** (`device_from_ua`, coarse Mobile/Desktop/Other, already on main). **OS/browser rules are deferred** (need the finer parsers from #3, which are not on main) — noted follow-up.

## Hot path
- Links WITHOUT rules (default, and every existing link) pay only a `Vec::is_empty()` check on the redirect — unchanged, no new cost.
- Links WITH rules pay rule evaluation: it REUSES the `country`/`user_agent` already read for the analytics `ClickEvent` (no extra header reads), parses the UA once via `device_from_ua`, and does a few string compares. NO store I/O. First matching rule wins; no match → the link's default `url`.

## Decisions (locked, user delegated)
- `Record.rules: Vec<Rule>` with `#[serde(default)]` (persisted; LMDB serde + Postgres `rules JSONB NOT NULL DEFAULT '[]'` column + migration + old-blob regression — the recurring lesson).
- `Rule { field: RuleField, values: Vec<String>, to: String }`; `RuleField = Country | Device` (serde lowercase). Match = the visitor's value is in `values` (country compared uppercase 2-letter; device is Mobile/Desktop/Other).
- `resolve_destination(rec, country: Option<&str>, ua: Option<&str>) -> &str`: if `rec.rules` empty → `&rec.url`; else first rule whose field-value matches → `&rule.to`; else `&rec.url`. Pure, unit-tested, used in the redirect handler (destination for the 302 LOCATION).
- `create`/`patch` accept optional `rules`. **SSRF: each `rule.to` MUST pass the same validation as the main url** (`is_valid_url` + `extract_host` + `is_internal_host`/blocklist) at create/patch time — a rule destination cannot be an internal/blocked host. Cap 20 rules/link. Normalize country to uppercase, device to the canonical set; reject unknown field/device.
- LinkRow exposes `rules`.

## Tasks
### Task 1 — backend: Record.rules + resolve_destination + redirect + validation
Files: `src/store/mod.rs` (Record + `Rule`/`RuleField`), lmdb.rs, postgres.rs (rules JSONB + migration + all Record sites), `src/api.rs` (`resolve_destination`, redirect uses it, create/patch accept+validate rules incl. SSRF per rule.to, LinkRow.rules), tests.
- Tests: a link with a Country rule (BR→url A) redirects a BR visitor (cf-ipcountry: BR) to A and others to the default; a Device rule (Mobile→url M) via a mobile UA; first-match ordering; no-rules link unchanged (302 to url, `resolve_destination` returns url); **SSRF: create/patch with a rule.to = http://127.0.0.1 → 400**; cap 20 → 400; **regression: old Record blob without rules → []**; gated Postgres rules round-trip. Existing redirect tests unchanged.

### Task 2 — frontend: rules editor + docs
Files: CreateLinkDialog/EditLinkDialog (a "Rules" section: add/remove rows of {field select, values input, destination url}; the base url is the default), LinkTable (a badge/indicator when a link has rules), types/i18n; `docs/REDIRECT-RULES.md`+`.PT_BR.md`, README/ROADMAP.
- The rules editor is optional/collapsible. Each row: field (Country/Device), values (comma-separated, e.g. `BR,PT` or `Mobile`), destination URL. i18n EN+PT. Vitest. Docs explain the model, first-match, the default, geo needs the edge to send `cf-ipcountry`. No em-dashes.

## Global constraints
- Common redirect (no rules) pays only `Vec::is_empty()` — no new cost; rule eval does NO store I/O and reuses the already-read country/UA.
- **Every rule destination passes the SSRF/blocklist validation** (create AND patch) — a rule cannot smuggle an internal destination.
- Record.rules persisted → serde(default) + Postgres migration + old-blob regression.
- All code English; UI i18n EN+PT; docs EN+PT_BR, no em-dashes. Rust `-j1`; gated skips clean. Stay on feat/geo-redirects; no merge.

## Out of scope
- OS/browser rules (need #3's parsers; follow-up).
- Weighted/percentage split (that is A/B testing, roadmap #17).
- City-level geo rules (country only; city needs the edge header from #3-style capture).
