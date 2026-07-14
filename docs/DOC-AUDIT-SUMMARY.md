# Doc audit summary

A record of the documentation pass that regenerated the docs from source at the
final project state (the 14 shipped features plus the scale-hardening layer:
`src/invalidate.rs`, atomic Postgres analytics counters, the durable webhook
outbox and relay in `src/webhooks/delivery.rs`, and the cluster preflight in
`src/cluster.rs`). Every claim was checked against `src/*.rs`, not against the
prior docs. This is a maintenance log, so it has no `.PT_BR.md` twin.

## Created

- `docs/API.md` and `docs/API.PT_BR.md`: full HTTP reference for every route in
  `src/api.rs` (`router_with_cors`): method, required scope, request/response
  shape, and status codes, including the shared auth failure table.
- `docs/CONFIGURATION.md` and `docs/CONFIGURATION.PT_BR.md`: every `QUARK_*`
  environment variable from `src/main.rs`, `src/cluster.rs`, `src/store/mod.rs`,
  and `src/api.rs`, with defaults and purpose, plus the baked-in constants.
- `docs/DEVELOPMENT.md` and `docs/DEVELOPMENT.PT_BR.md`: build/run, the Docker
  Postgres/Valkey/ClickHouse stack, the `QUARK_TEST_*` gated integration tests,
  and why they run single-threaded.

## Rewritten

- `docs/ARCHITECTURE.md` and `.PT_BR.md`: the module table now lists every module
  (added `auth`, `pixel`, `import`, `webhooks`, `invalidate`, `cluster`); the
  `Record` shows all nine fields (was three); the LMDB section documents eleven
  named databases (was six); a Postgres data-model section covers the thirteen
  tables and four sequences; the redirect flow states the app > rule > variant
  precedence and `max_visits`; a new section covers the scale-hardening layer.
- `docs/SCALING.md` and `.PT_BR.md`: the honest matrix now reflects atomic
  Postgres analytics counters (no longer "a per-link hotspot"); new rows and
  sections cover at-most-once click ingestion and durable webhook delivery
  (outbox + leased relay + retry/DLQ + idempotency; `link.clicked`/`link.expired`
  best-effort by design), the residual same-transaction follow-up, and a
  cross-link to `docs/research/2026-07-14-scale-audit.md`.

## Patched

- `README.md` and `README.PT_BR.md`: consolidated the duplicated Quick links
  line; added API, Configuration, and Development to the docs lists; noted
  durable webhook delivery; pointed the config table at the full reference. The
  Portuguese README also had its prose em-dashes removed.
- `docs/ROADMAP.md` and `.PT_BR.md`: the device-aware redirect and the whole
  scale-hardening layer moved from "follow-up" to Done; webhooks are now durable
  on Postgres; the backlog reflects the remaining deep-linking and
  same-transaction-outbox items.
- `docs/DEPLOY.md` and `.PT_BR.md`: the operating note now points multi-node at a
  shared Postgres + Valkey with `QUARK_STRICT_CLUSTER`, replacing the stale
  "that's phase 2" line.

## Verified current (no change needed)

The feature docs and their twins already matched the final code and were left as
they were: `WEBHOOKS`, `ANALYTICS`, `CONVERSION-FORWARDING`, `REDIRECT-RULES`,
`AB-TESTING`, `DEEP-LINKING`, `API-TOKENS`, `IMPORT`, `EDGE`, `CONTRIBUTING`.
Each was checked field by field against the source (webhook events and signing,
the atomic counters and the blob-not-migrated caveat, the GA4/Meta dedup keys,
the rule and variant caps, the well-known paths, the token scopes, the import
column mapping).

## Prose self-check

No em-dashes (the long dash, or a double hyphen) remain in any `docs/*.md` or the
READMEs; the only double-hyphens are CLI flags inside code fences and Mermaid
arrow labels. No
AI-isms (seamless, robust, leverage, comprehensive, delve, ensure, cutting-edge,
"it's worth noting") in the new or rewritten prose.

## Gaps not filled

- The historical `docs/specs/*` and `docs/plans/*` design records were left as
  written. They are dated design notes, not living reference docs, so they still
  contain their original prose (including em-dashes).
- The scale audit `docs/research/2026-07-14-scale-audit.md` is a point-in-time
  Portuguese record and was left intact; the closed gaps it lists are now
  documented as done in `SCALING.md` and `ROADMAP.md`, which cross-link to it.
