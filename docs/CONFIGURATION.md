**English** · [Português](CONFIGURATION.PT_BR.md)

# Configuration reference

Every quark setting is an environment variable read once at startup. There is
no config file and no build-time feature flag: which backends run is decided
purely by which `QUARK_*` variables are set. This page lists every variable the
binary reads, its default, and what it does. The source of truth is
`src/main.rs`, plus `src/cluster.rs` (the cluster preflight), `src/store/mod.rs`
(backend selection), and `src/api.rs` (CORS).

Only `QUARK_KEY` matters for a real deployment. Leave everything else unset and
quark runs as a single zero-dependency binary on `0.0.0.0:8080` with an LMDB
store.

## Core

| Variable | Default | Purpose |
|---|---|---|
| `QUARK_KEY` | dev fallback `11400714819323198485` (loud warning) | The permutation key, parsed as a **decimal** `u64`. It is what makes the code space unpredictable per instance. Set a random value in production and keep it out of source control. A hex string will not parse and silently falls back to the dev key. |
| `QUARK_SIGNING_KEY` | random per process (loud warning) | Base64 secret (>= 32 bytes) that signs link-password unlock cookies, kept separate from `QUARK_KEY`. Unset means a fresh random key each start, so unlock cookies do not survive a restart and are not shared across nodes. Set it (and share it across replicas) for multi-node or persistent deployments. Only relevant if you use password-protected links. |
| `QUARK_HEALTH_CHECK_SECS` | unset (disabled) | Enables broken-link monitoring: seconds between destination-health sweeps (clamped up to 60). Lease-coordinated (Postgres); safe to enable on all replicas. See [LINK-HEALTH](LINK-HEALTH.md). |
| `QUARK_ADDR` | `0.0.0.0:8080` | HTTP bind address. |
| `QUARK_DATA` | `./data` (container image: `/data`) | LMDB data directory, created if missing. Only used when the store is LMDB (unset `QUARK_DATABASE_URL`). |

Generate a key with `od -An -N8 -tu8 /dev/urandom | tr -d ' '`. Changing
`QUARK_KEY` remaps the entire code space, so every already-issued code stops
resolving. Keep it stable once links exist.

## Backends

Each backend is opt-in and selected independently. The store follows
`QUARK_DATABASE_URL`; the analytics sink follows `QUARK_CLICKHOUSE_URL` if set,
otherwise it is the store's own embedded sink; the L2 cache and the shared
control connection follow `QUARK_VALKEY_URL`.

| Variable | Default | Purpose |
|---|---|---|
| `QUARK_DATABASE_URL` | unset (LMDB) | Use Postgres for the store, e.g. `postgres://user:pass@host:5432/db`. Postgres is the shared, multi-node-safe store and also implements the analytics sink. Unset falls back to the embedded LMDB store. |
| `QUARK_VALKEY_URL` | unset (L1 + store only) | Enable the L2 Valkey cache, e.g. `redis://host:6379`. The same connection also backs the global rate limit and the cross-node invalidation pub/sub. |
| `QUARK_CLICKHOUSE_URL` | unset (store's embedded sink) | Use ClickHouse for the analytics sink, e.g. `http://user:pass@host:8123/db`. ClickHouse is analytics-only; it never becomes the link store. |
| `QUARK_NODE_ID` | unset (full 40-bit id space) | LMDB-only id-space partitioning, `0`-`255`. The top 8 bits become the node id and the low 32 bits a node-local counter. Ignored on the Postgres backend (the shared sequence handles allocation) and quark logs that it was ignored. An out-of-range value crashes the process at startup. See [SCALING](SCALING.md). |

`QUARK_NODE_ID` partitions the id space so codes never collide between LMDB
nodes; it does not make separate LMDB files share links. Real multi-node needs
Postgres. The id must be unique per replica and quark cannot detect a duplicate.

## Cluster preflight

| Variable | Default | Purpose |
|---|---|---|
| `QUARK_STRICT_CLUSTER` | unset (off) | When set to any non-empty value, quark refuses to start unless both `QUARK_DATABASE_URL` and `QUARK_VALKEY_URL` are present, and names the missing one. This turns a silent multi-node misconfiguration (per-node LMDB files, N-times rate limits, stale caches) into a startup error. Leave unset for single-node. |

The check is `cluster_preflight` in `src/cluster.rs`. When strict is off it always
passes and single-node behavior is untouched.

## Admin and access

| Variable | Default | Purpose |
|---|---|---|
| `QUARK_ADMIN_TOKEN` | unset | The operator token, sent in the `x-admin-token` header. It always behaves as the `full` scope. When unset, `POST /` stays a public open shortener and every `/admin/*` endpoint plus `GET /:code/stats` answers `404` (fully disabled). When set, those endpoints require it or a scoped API token, and `POST /` requires a token that covers `links_write`. See [API-TOKENS](API-TOKENS.md). |
| `QUARK_CORS_ORIGINS` | unset (same-origin only) | Comma-separated list of origins allowed to call the API, for the separately hosted web panel. Empty means no CORS layer. |
| `QUARK_ACCESS_LOG` | unset (off) | Enable a per-request JSON access log line (`{"method","path","status","latency_ms"}`) on stdout. Off by default so the redirect hot path pays no synchronous stdout cost. |

## Abuse protection

These apply to `POST /` only. The redirect path is never touched by them.

| Variable | Default | Purpose |
|---|---|---|
| `QUARK_RATELIMIT_PER_MIN` | unset / `0` (off) | Creations per minute per client IP on `POST /`, a fixed 60-second window. With `QUARK_VALKEY_URL` set it is a global limit across replicas (Valkey `INCR`/`EXPIRE`); otherwise it is in-memory per replica. Fail-open: a Valkey error lets the request through. |
| `QUARK_REAL_IP_HEADER` | `cf-connecting-ip` | Header to read the client IP from, with a socket-address fallback. Because the header is trusted, only enable the rate limit behind a proxy that overwrites it, or a client can forge it. |
| `QUARK_BLOCK_PRIVATE` | on (set `0` to disable) | The internal/loop guard. Rejects a destination whose host is a private, loopback, or link-local IP literal (v4 and v6, including IPv4-mapped like `::ffff:127.0.0.1`), `localhost`, or the instance's own host. It never resolves DNS. |
| `QUARK_PUBLIC_HOST` | unset (uses the `Host` header) | This instance's own host, used by the anti-loop check so a link cannot point back at quark itself. |

## Defaults baked into the binary

These are compile-time constants, not environment variables, but they bound
behavior and are useful when sizing a deployment. All live in `src/main.rs`
unless noted.

| Constant | Value | What it bounds |
|---|---|---|
| L1 cache capacity | 100,000 records | Max `id -> Record` entries held in the moka L1 cache per process. |
| L1 cache TTL | 60s (`src/cache/mod.rs`) | How long an L1 entry lives before reload. Also the staleness backstop across nodes. |
| L2 cache TTL | 3600s (`src/cache/mod.rs`) | Valkey L2 entry TTL, capped shorter for a link near its expiry. |
| L2 op timeout | 100ms (`src/cache/mod.rs`) | Per-op bound on a Valkey call; a timeout counts as a breaker failure. |
| Analytics channel capacity | 10,000 events | Buffered `ClickEvent`s before the redirect's `try_send` drops (at-most-once ingestion). |
| Webhook channel capacity | 1,024 events (`src/webhooks/delivery.rs`) | In-memory best-effort webhook queue depth. |
| Id space width | 40 bits (`src/permute.rs`) | `MAX_ID = 2^40 - 1`, about 1.1 trillion links. |
| Import row cap | 10,000 rows (`src/import.rs`) | Max rows per `POST /admin/import` request. |

## Related pages

- [Deploy on Coolify](DEPLOY.md) shows these variables in a real deployment.
- [Development](DEVELOPMENT.md) covers the local Docker stack and the gated
  integration tests keyed on `QUARK_TEST_*` variables.
- [Scaling](SCALING.md) explains `QUARK_STRICT_CLUSTER`, `QUARK_NODE_ID`, and
  the single-node versus multi-node matrix.
