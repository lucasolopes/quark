**English** · [Português](ROADMAP.PT_BR.md)

# quark roadmap

## Current state

Essentially complete single-operator OSS product: **API + web panel**, under an
**AGPL-3.0 license** with a **CLA** for contributions. Single-binary,
zero-dependency core by default; network backends are opt-in via env vars
(chosen at startup, no build-time feature flag). Tested (57 lib tests + 29 API
tests + a gated Postgres/Valkey/ClickHouse integration suite, incl. search +
34 frontend tests) and benchmarked (permute ~264M ops/s; redirect ~7.9µs
in-process; in production it scaled linearly up to 1k VUs, with the measured
bottleneck being geography/RTT, not the server).

## Done

- **Core (v0.1):** create + redirect + custom alias + expiration (TTL). The short code
  is a calibrated Feistel/ARX permutation (`ROUNDS=4`); codes are **computed, not
  stored** (store keyed by `u64`).
- **Pluggable architecture**: `Store` / `CacheTier` / `AnalyticsSink` traits:
  - **L2 Valkey** (`QUARK_VALKEY_URL`): shared cache, circuit breaker + timeout, fail-open.
  - **Postgres** (`QUARK_DATABASE_URL`): multi-node relational store (atomic id sequence).
  - **ClickHouse** (`QUARK_CLICKHOUSE_URL`): OLAP analytics sink (analytics-only).
- **Click analytics**: fire-and-forget capture on the 302 (~180ns) → worker → sink;
  `GET /:code/stats` (aggregates + last N events).
- **Observability**: opt-in per-request JSON access log (`QUARK_ACCESS_LOG`).
- **Edge/CDN**: TTL-aware `Cache-Control` on redirects (guide in `docs/EDGE.md`).
- **Horizontal scaling**: stateless replicas over a shared Postgres; `QUARK_NODE_ID`
  partitions the id space in LMDB (defensive guard). Doc: `docs/SCALING.md`.
- **Abuse protection** (only on `POST /`): per-IP rate limit (`QUARK_RATELIMIT_PER_MIN`,
  in-memory/Valkey, fail-open), destination blocklist in the store (`/admin/blocklist`, L1/L2 cache),
  built-in guard against internal/loop networks (`QUARK_BLOCK_PRIVATE`, on by default).
- **Panel API**: `GET /admin/links` (keyset-paginated list), `DELETE`/`PATCH /admin/links/:code`,
  all under `QUARK_ADMIN_TOKEN`. **Creating (`POST /`) requires the token when `QUARK_ADMIN_TOKEN`
  is set** (otherwise it stays public). Opt-in CORS via `QUARK_CORS_ORIGINS`.
- **Web panel (SPA)**: `web/` (React + Vite + shadcn/ui + TanStack + Recharts), deployed
  separately (static build), API-only binary. Token login → Links (CRUD, search,
  copy, **QR code**) → Per-link stats (charts) → Blocklist. UI/UX following
  Nielsen's heuristics.
- **Server-side search (Postgres)**: `GET /admin/links?q=` runs `ILIKE` over url+alias
  (keyset-paginated, wildcards escaped). A Postgres-only feature; LMDB returns `501` and the
  panel falls back to **client-side** filtering (~300ms debounce, automatic fallback). A distinct
  error state from "nothing found".
- **License + contributions**: **AGPL-3.0-only** core; `CLA.md` (license grant) +
  `CONTRIBUTING.md` + a CLA bot (GitHub Action). Multi-tenancy/cloud stays proprietary, separate.
- **`docker-compose.yml`**: full stack (quark + Postgres + Valkey + ClickHouse) for dev/self-host.
- **Importer (#4)**: `POST /admin/import` bulk-creates links from a CSV or JSON export (Bitly, Kutt,
  YOURLS, generic), partial-success per-row report, plus a web panel "Import" tab. Doc:
  [`docs/IMPORT.md`](IMPORT.md).

## Next

- **Accounts + multi-user panel**: this is a **cloud-phase** feature (multi-tenant, proprietary). OSS
  stays single-account (single operator). Deserves its own brainstorming when the time comes.
- **Deploying the full version on the VPS**: the API + panel aren't in production yet
  (`quark.meuchat.ai` runs an older version); bring it up via Coolify.

## Backlog

- **Custom domains**: `mydomain.com/abc`.

## Design constraints (deliberate)

- **A pure binary (LMDB, no database) is single-node by design**: this is not a limitation to remove.
  Scaling means stateless replicas over a shared Postgres (`docs/SCALING.md`).
- **Abuse protection** runs only on `POST /`; the redirect (hot path) pays nothing for it.
- **Creating a link is public when there's no `QUARK_ADMIN_TOKEN`** (a zero-config open shortener);
  setting the token locks creation down to the operator.

## Parked (future, not planned)

- **Full-edge cloud on Cloudflare Workers**: the direction for the cloud edition (permute compiled to WASM;
  Store becomes KV/D1/Durable Objects). Parked until it becomes a priority.
- **Shared-nothing proxy** (multi-node LMDB without a database): not planned; Postgres already covers multi-node.

## Notes

- Anti-abuse, horizontal scaling, analytics, and the panel **have been delivered** (no longer future items).
- GitHub CI has a Rust job (with valkey+postgres+clickhouse services for the gated tests) and
  a `web` job (frontend lint/typecheck/test/build). The CLA is collected by a bot on every PR.
