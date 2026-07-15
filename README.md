**English** · [Português](README.PT_BR.md)

# quark

[![CI](https://github.com/lucasolopes/quark/actions/workflows/ci.yml/badge.svg)](https://github.com/lucasolopes/quark/actions/workflows/ci.yml)
![License: AGPL v3](https://img.shields.io/badge/license-AGPL--3.0-blue)
![Rust 2021](https://img.shields.io/badge/rust-2021-orange)
![Runtime deps: none](https://img.shields.io/badge/runtime%20deps-none-brightgreen)
![Self-hosted](https://img.shields.io/badge/self--hosted-single%20binary-informational)

> Short codes are **computed, not stored**: a keyed bijection. One tiny static binary (~1 MB), no Redis, no database, no external services.

**Quick links:** [API](docs/API.md) · [Configuration](docs/CONFIGURATION.md) · [Architecture](docs/ARCHITECTURE.md) · [Deploy](docs/DEPLOY.md) · [Scaling](docs/SCALING.md) · [Development](docs/DEVELOPMENT.md) · [Roadmap](docs/ROADMAP.md)

A URL shortener whose short code is a **calibrated, reduced-round ARX permutation** of the internal integer id. The code is not looked up in an index. It is **computed**, in both directions, from a tiny bijective function. That one design choice removes an entire class of problems (collisions) and an entire index (string → id) at once.

## The pitch

Most shorteners pick one of two paths for the code:

- **Reversible encoding** (Hashids, Sqids-style): fast, but not security. Codes are partially enumerable; you can scrape `/aaaa`, `/aaab`, …
- **Real cipher** (e.g. Feistly = Feistel + HMAC-SHA256): non-enumerable, but slow. A full cryptographic hash runs on every round.

quark closes that gap with a **Feistel network whose round function is ARX** (add-rotate-xor), not a hash. A Feistel network over an integer domain is a bijection by construction: `decode(encode(id)) == id` for every id, with **zero collision checks needed**, ever. The only open question is *how many rounds* of mixing are needed before the output looks random enough to resist enumeration, and that's not a guess here: it's **measured** (see the avalanche table below). The result is a code generator that is simultaneously non-enumerable *and* roughly an order of magnitude faster than a real-cipher approach (~18× measured against a structurally identical HMAC-round Feistel, see the benchmark below), because ARX rounds are cheap integer ops, not hash calls.

Since the code is the permutation of the id, the store never has to index by string. It's keyed by `u64`, straight into an mmap'd database. Millions of links occupy a fraction of what a string-indexed store would need.

## Architecture

```mermaid
flowchart LR
    C[Client] -->|POST /| API[api axum]
    C -->|GET /:code| API
    API -->|encode/decode| P[permute + codec]
    API --> CA[cache moka]
    CA -->|miss| ST[(store LMDB)]
    P -.no I/O.-> API
```

`permute` (the Feistel/ARX bijection) and `codec` (integer ↔ base62) are pure math: no I/O, no locks, off to the side of the request path. The hot path is: decode the code to an id, check the in-memory cache, fall back to a single mmap read on miss.

## Redirect sequence

```mermaid
sequenceDiagram
    participant Cli as Client
    participant Api as api
    participant Ca as cache
    participant St as store
    Cli->>Api: GET /:code
    Api->>Api: id = decode(base62⁻¹(code), key)
    Api->>Ca: get(id)
    alt hit
        Ca-->>Api: Record
    else miss
        Ca->>St: get_link(id)
        St-->>Ca: Record | None
        Ca-->>Api: Record | None
    end
    alt found and not expired
        Api-->>Cli: 302 Location: url
    else expired
        Api-->>Cli: 410 Gone
    else not found
        Api-->>Cli: 404
    end
```

Numeric base62 codes are resolved first, by pure arithmetic (masked, never panics). Only a code that is **not** a valid in-range 7-char base62 string (wrong length, an invalid character, or a value greater than `MAX_ID`) falls through to a custom alias lookup in the store.

### Aliases

A custom `alias` must not itself be a valid 7-char base62 code in range `0..=MAX_ID`: if it were, it would be indistinguishable from a computed numeric code and would be unreachable (shadowed by the numeric branch above). `create` rejects such aliases with `400 Bad Request` at creation time, before allocating an id, so they never make it into the store.

## How many rounds? Measured, not guessed

The round count for the Feistel/ARX permutation isn't picked by intuition; it's calibrated with an avalanche/SAC (Strict Avalanche Criterion) harness (`cargo run --bin calibrate`), a direct port of the diffusion measurement tooling built for a SHA-256 research lab. The idea behind SAC is simple: **flip one bit of the input id, and on a well-mixed permutation, about half the output bits should flip, unpredictably.** If flipping bit 5 of the id always flips the same 3 output bits, the code is enumerable. If it flips ~50% of the bits, on average, no matter which input bit you flip, the output looks like noise from the outside.

Measured result, sweeping 1 to 12 rounds over 200,000 random samples per round:

```
rounds | avalanche_medio | cobertura(/40)
   1   |     0.1381      |    1
   2   |     0.3622      |   21
   3   |     0.4866      |   40
   4   |     0.5000      |   40   ← ROUNDS escolhido (difusão fecha)
 5..12  |     0.5000      |   40
```

- **avalanche_medio**: average fraction of output bits that flip when one input bit flips (target: 0.5 exactly).
- **cobertura**: the minimum, across all 40 input bits, of how many distinct output bits that single input bit has ever managed to affect. `40/40` means every input bit can influence every output bit: full diffusion, no structural blind spot.

`ROUNDS = 4` is the smallest round count where avalanche hits `0.5000` exactly *and* coverage is full. Round 3 is close (`0.4866`) but not there yet. Rounds 5 through 12 buy nothing: the diffusion has already closed, so quark uses 4 and stops, keeping every round of runtime that isn't needed for the property it's paying for.

## Speed: the trophy number

```
cargo bench --bench permute_bench
```

Measured on this machine (criterion, `benches/permute_bench.rs`):

| op | time/op | ops/sec |
|---|---|---|
| `permute::encode` (u64 → u64, the permutation engine) | ~3.98 ns | ~251,000,000 |
| `permute::decode` (u64 → u64) | ~3.45 ns | ~290,000,000 |

That's the raw permutation. The **product operation** is `id → 7-char base62 string` (and back), which adds one heap-allocated `String` per call:

| op | time/op | ops/sec |
|---|---|---|
| `encode` (id → code string) | ~45 ns | ~22,000,000 |
| `decode` (code string → id) | ~80 ns | ~12,500,000 |

### Head-to-head (same machine, same criterion harness)

`benches/compare_bench.rs` measures the same class of operation (*integer id → opaque short string*) for quark and three real competitor approaches, over ids in `0..2^40`. The one that isolates quark's actual claim is **`feistel_hmac`**: an identical balanced Feistel (4 rounds, 40 bits), changing *only* the round function from ARX to HMAC-SHA256 (i.e. the "real cipher" approach that libraries like Feistly take).

| approach | encode ops/sec | vs quark | what it is |
|---|---|---|---|
| **quark** (ARX Feistel) | **~22,000,000** | 1× | keyed bijection, fixed 7-char, non-enumerable |
| hashids (`harsh` 0.2.2) | ~2,950,000 | ~7.5× slower | obfuscation encoding (weak salt, not keyed) |
| feistel + HMAC-SHA256 | ~1,230,000 | **~18× slower** | same structure as quark, hash round fn |
| sqids (`sqids` 0.4.2) | ~680,000 | ~32× slower | obfuscation encoding (no key) |

The honest headline: against the **structurally identical** cipher (same Feistel, keyed, only the round function differs), quark's ARX round is **~18× faster**, because each round is a handful of adds/rotates/xors, not a cryptographic hash invocation. That is the direct payoff of *measuring* the minimum round count (4) instead of over-provisioning "for safety".

**Fairness caveat:** sqids and hashids are obfuscation encodings: they hide sequential ids but are **not** keyed cryptographic primitives (sqids has no key; hashids' salt is documented as non-secure), and they encode an arbitrary-length domain rather than quark's fixed 40-bit → 7-char bijection. So against those two, quark's numbers show a speed advantage, not a security-equivalent one. Only `feistel_hmac` is a like-for-like security comparison. Reproduce all of it with `cargo bench --bench compare_bench`.

### Redirect capacity (end-to-end HTTP)

The numbers above are the code generator in isolation. Serving an actual redirect adds the HTTP stack + cache lookup. Two measurements, because they answer two different questions.

**Production, from an external client.** A real deploy on a VPS in Germany (Coolify + Traefik + TLS), load-tested with k6 from a client in Brazil, the full path a real user travels. `GET /:code`, not following the 302, so it measures quark, not the destination. Reproduce with `scripts/loadtest.k6.js`.

| concurrent users (VUs) | throughput | median | p95 | errors |
|---|---|---|---|---|
| 100 | 374 req/s | 214 ms | 228 ms | **0%** |
| 500 | 1,844 req/s | 218 ms | 235 ms | **0%** |
| 1,000 | 3,399 req/s | 221 ms | 262 ms | **0%** |

How to read it: **throughput scales almost linearly with concurrency** (10× the users → 9.1× the requests/sec) while **latency stays essentially flat** (+7 ms median from 100 to 1,000 concurrent users): the signature of a server that never queues. **0 errors across ~225k requests**, 100% correct 302s. The ~214 ms is **not** quark: it is the São Paulo↔Germany round trip (~200 ms of transatlantic RTT). Idle latency was ~212 ms, so quark adds ~2 ms *even under 1,000 concurrent users*. A user near the VPS (e.g. in Europe) sees that same ~2 ms of server work plus their own much shorter RTT (tens of ms). The bottleneck we hit was how much one distant client could pull, **not the server**; its real ceiling is higher and can only be found with distributed load.

**Local capacity proxy (no network in the way).** Hitting the release container on one dev machine (RTT ≈ 0, cache-hit hot path) with `oha`: **~124k req/s @ 50 conns** (p50 0.33 ms, p99 1.4 ms) and **~152k req/s @ 200 conns** (p50 0.90 ms, p99 6.3 ms). This was inside a Docker Desktop VM (capped CPU), so it is a *floor* on raw capacity, not a ceiling. Even so, ~150k redirects/sec is ~13 billion/day, orders of magnitude past the "millions/day" target.

Both measurements point to the same conclusion: **the redirect path is never the limiting factor.** What a user experiences is dominated by their network distance to the VPS, which is a geography problem (solved by an edge/CDN in front), not a quark problem.

## Backends & scaling

quark's persistence, cache, and analytics are each behind a trait (`Store`, `CacheTier`, `AnalyticsSink`). The default is a **zero-dep single binary**; every other backend is **opt-in**, selected purely by which env var is set. Nothing here changes the hot path's shape; it changes what's on the other side of it.

- **Default: LMDB store + L1 in-process cache + embedded LMDB analytics sink.** No external service, no config. `docker run`, done. This is what you get with none of the vars below set.
- **L2 cache: Valkey, via `QUARK_VALKEY_URL`.** A shared L2 cache across multiple quark instances (the L1 `moka` cache is per-process). Circuit-breaker and a 100ms per-op timeout protected: a down or hung Valkey never blocks a redirect; it just falls back to L1/store. *Why:* horizontal scaling without every instance cold-starting its own cache.
- **Relational store: Postgres, via `QUARK_DATABASE_URL`.** Implements both `Store` (links, aliases, an atomic id sequence) and `AnalyticsSink`. Unset → LMDB. *Why:* multi-node-safe persistence when a single embedded LMDB file can no longer be the source of truth for more than one instance.
- **Analytics sink: ClickHouse, via `QUARK_CLICKHOUSE_URL`.** OLAP-style append + aggregate-by-query, for high-volume click analytics. Unset → whichever store is active provides its own embedded sink. ClickHouse is analytics-only; it never becomes the link store. *Why:* click event volume can be orders of magnitude higher than link-create volume, and wants a column store, not the same engine serving redirects.

Store and AnalyticsSink are selected **independently**: e.g. Postgres store + ClickHouse analytics, or Postgres store + its own embedded analytics, are both valid combinations.

- **Webhooks**: signed outgoing HTTP events (`link.created/updated/deleted/expired/clicked`) to any endpoint (Zapier, Make, n8n, Slack, custom). On Postgres the lifecycle events are delivered durably (outbox + leased relay + retry/DLQ); `link.clicked`/`link.expired` stay best-effort by design. Config persisted via `Store`. See [`docs/WEBHOOKS.md`](docs/WEBHOOKS.md).

The framing: quark **scales down to one binary with zero external dependencies**, and **scales up to a distributed stack** (Valkey + Postgres + ClickHouse) **one opt-in piece at a time**, never all-or-nothing. Compare that to heavier shorteners (e.g. Dub) that require Postgres + Redis + ClickHouse from day one, even for a single low-traffic instance.

```mermaid
flowchart LR
    Cli[Client] -->|GET /:code| Api[api axum]
    Api --> L1[L1 moka in-process]
    L1 -->|miss| L2{L2 Valkey opt-in}
    L2 -->|hit| Api
    L2 -->|miss or unset| Store[(Store: LMDB or Postgres)]
    Store --> Api
    Api -->|302| Cli
    Api -.click event.-> Chan[[channel]]
    Chan --> Worker[analytics worker]
    Worker --> Sink[(Sink: LMDB or ClickHouse)]
```



```bash
# QUARK_KEY is parsed as a DECIMAL u64 (not hex). Generate one:
export QUARK_KEY=$(od -An -N8 -tu8 /dev/urandom | tr -d ' ')
export QUARK_DATA=./data        # LMDB directory, created if missing
export QUARK_ADDR=0.0.0.0:8080  # bind address
cargo run --release
```

If `QUARK_KEY` isn't set, or isn't a valid decimal `u64` (a hex string will silently fail to parse), quark logs a loud warning and falls back to a hardcoded dev key. Fine for local testing, **never for production**: the key is what makes the code space unpredictable per instance.

### curl examples

```bash
# create a short link
curl -X POST localhost:8080/ -H 'content-type: application/json' \
  -d '{"url": "https://example.com/some/very/long/path"}'
# => {"code":"01aB2Cd","url":"https://example.com/some/very/long/path"}

# create with a custom alias and a 1-hour TTL
curl -X POST localhost:8080/ -H 'content-type: application/json' \
  -d '{"url": "https://example.com", "alias": "promo", "ttl": 3600}'

# create a link that expires after 100 visits instead of (or alongside) a TTL
curl -X POST localhost:8080/ -H 'content-type: application/json' \
  -d '{"url": "https://example.com", "max_visits": 100}'

# follow it
curl -i localhost:8080/01aB2Cd   # -> 302 Location: https://example.com/...

# health check
curl localhost:8080/health
```

## Threat model: read this before relying on it for secrecy

quark's non-enumerability is a **measured statistical property** (avalanche/SAC over a reduced-round ARX permutation), not a cryptographic guarantee. It resists casual scraping and sequential guessing far better than a raw counter or Hashids-style encoding, and changing `QUARK_KEY` remaps the entire code space. But this is **not AES**, and it is **not** a substitute for real access control if the linked resource itself needs to stay secret. Treat codes as "hard to guess by brute force in practice," not "cryptographically secret." Each instance should run with its own random `QUARK_KEY`, kept out of source control.

## Configuration

Every var below is optional except `QUARK_KEY` in production. Unset a backend var and quark falls back to the zero-dep default. This table is the common subset; the complete reference (including `QUARK_STRICT_CLUSTER`, `QUARK_NODE_ID`, and the baked-in defaults) is [`docs/CONFIGURATION.md`](docs/CONFIGURATION.md).

| Var | Purpose | Default |
|---|---|---|
| `QUARK_KEY` | Decimal `u64` secret, the permutation key. **Required in production** (each instance should have its own, kept out of source control). | dev fallback key (loud warning logged; not for production) |
| `QUARK_DATA` | LMDB data directory. Only used when the store is LMDB. | `./data` (container: `/data`) |
| `QUARK_ADDR` | HTTP bind address. | `0.0.0.0:8080` |
| `QUARK_ADMIN_TOKEN` | Enables the token-protected admin endpoints, e.g. `GET /:code/stats`. | unset → those endpoints off (404) |
| `QUARK_VALKEY_URL` | Enable the L2 Valkey cache, e.g. `redis://host:6379`. | unset → L1 + store only |
| `QUARK_DATABASE_URL` | Use Postgres for the store, e.g. `postgres://user:pass@host:5432/db`. | unset → LMDB |
| `QUARK_CLICKHOUSE_URL` | Use ClickHouse for analytics, e.g. `http://user:pass@host:8123/db`. | unset → store's embedded sink |
| `QUARK_ACCESS_LOG` | Enable per-request JSON access log on stdout. | unset → off |
| `QUARK_RATELIMIT_PER_MIN` | Creations/min per IP on `POST /` (unset/`0` = off). Uses Valkey if `QUARK_VALKEY_URL` is set (global limit), else in-memory per replica. | unset → off |
| `QUARK_REAL_IP_HEADER` | Header to read the client IP from. | `CF-Connecting-IP` |
| `QUARK_BLOCK_PRIVATE` | Guard against internal/loop destinations; on by default, `0` disables it. | on |
| `QUARK_PUBLIC_HOST` | This instance's own host, for anti-loop (otherwise uses the `Host` header). | unset → uses `Host` header |
| `QUARK_CORS_ORIGINS` | Comma-separated origins allowed to call the API (for the web panel). | unset → no CORS (same-origin only) |

> Only enable `QUARK_RATELIMIT_PER_MIN` behind a proxy that overwrites `QUARK_REAL_IP_HEADER` (e.g. Cloudflare with `CF-Connecting-IP`); exposed directly, a client can forge the header and bypass the limit.

## Operating

- Per-request access logging is **opt-in via `QUARK_ACCESS_LOG`** (off by default). When set, every request emits a **structured JSON log line** to stdout (`{"method","path","status","latency_ms"}`), captured as-is by Coolify/Docker, ready to `grep` or ship to a log collector. Off by default so the hot redirect path pays no synchronous `println!`/stdout-lock cost at high throughput.
- Redirects carry a **TTL-aware `Cache-Control`** header, so a CDN/browser can cache the 302 (and never past a link's expiry). See [`docs/EDGE.md`](docs/EDGE.md) for putting Cloudflare in front.
- A link can also expire after a maximum number of visits (`max_visits`), in addition to or instead of a TTL date; the redirect returns `410 Gone` once the limit is reached.
- **Import**: `POST /admin/import` bulk-creates links from a CSV or JSON export (Bitly, Kutt, YOURLS, or generic), same admin token, partial-success reporting per row. See [`docs/IMPORT.md`](docs/IMPORT.md).
- Beyond the env admin token, named **API tokens** with per-permission scopes and an optional per-token rate limit can be managed under `/admin/tokens` (superuser scope only); see [`docs/API-TOKENS.md`](docs/API-TOKENS.md).

### Local dev stack

`docker compose up --build` brings up quark plus all three optional backends
(Postgres, Valkey, ClickHouse) wired together, handy for development, for
running the gated integration tests, and as a full-stack self-host reference.
The admin/panel API lives under `/admin/*` (token `QUARK_ADMIN_TOKEN`): list
links `GET /admin/links` (filterable by `?tag=`), delete `DELETE /admin/links/:code`, edit
`PATCH /admin/links/:code`, and list distinct tags `GET /admin/tags`. A separate web panel (SPA) consumes this API; set
`QUARK_CORS_ORIGINS` to the panel's origin.

### Web panel (`web/`)

A single-operator admin panel (React SPA) lives in `web/`. It's built and
deployed **separately** from the API binary (static build → CDN/edge); the quark
binary stays API-only. Links support CRUD, search, tags, copy, and QR code,
plus per-link stats. Dev: `cd web && npm install && npm run dev` (Vite on
`:5173`), pointing `VITE_API_BASE_URL` at your quark API and setting
`QUARK_CORS_ORIGINS=http://localhost:5173` on the API. Auth is the same
`QUARK_ADMIN_TOKEN`, entered on the panel's login screen. The create-link dialog
includes an optional UTM builder with locally saved templates (`localStorage`).

## More

- Deploy on a VPS with Coolify (ships a `Dockerfile`): [`docs/DEPLOY.md`](docs/DEPLOY.md)
- Edge/CDN caching guide: [`docs/EDGE.md`](docs/EDGE.md)
- Signed outgoing webhooks (events, payload, signature verification): [`docs/WEBHOOKS.md`](docs/WEBHOOKS.md)
- Click analytics and privacy posture (what's captured, what's never stored): [`docs/ANALYTICS.md`](docs/ANALYTICS.md)
- Migrating from Bitly/Kutt/YOURLS: [`docs/IMPORT.md`](docs/IMPORT.md)
- Geo/device redirect rules: [`docs/REDIRECT-RULES.md`](docs/REDIRECT-RULES.md)
- Server-side conversion forwarding (GA4/Meta CAPI, no client-side pixel): [`docs/CONVERSION-FORWARDING.md`](docs/CONVERSION-FORWARDING.md)
- Deep linking (hosting the iOS/Android app-association files): [`docs/DEEP-LINKING.md`](docs/DEEP-LINKING.md)
- What's next: [`docs/ROADMAP.md`](docs/ROADMAP.md)
- Full system design: [`docs/specs/2026-07-12-quark-design.md`](docs/specs/2026-07-12-quark-design.md)
- Deeper walkthrough of every component, data model and the Feistel round internals: [`docs/ARCHITECTURE.md`](docs/ARCHITECTURE.md)
- Full HTTP API reference (every route, scope, request/response shape, status codes): [`docs/API.md`](docs/API.md)
- Every `QUARK_*` environment variable with defaults and purpose: [`docs/CONFIGURATION.md`](docs/CONFIGURATION.md)
- Building, running, the Docker stack, and the gated integration tests: [`docs/DEVELOPMENT.md`](docs/DEVELOPMENT.md)
- Horizontal scaling (replicas + Postgres + Valkey) and `QUARK_NODE_ID`: [`docs/SCALING.md`](docs/SCALING.md)
- A/B testing (weighted variants + per-variant stats): [`docs/AB-TESTING.md`](docs/AB-TESTING.md)
- Password-protected links (argon2 + interstitial): [`docs/LINK-PASSWORD.md`](docs/LINK-PASSWORD.md)
- Broken-link monitoring (health checker + webhooks): [`docs/LINK-HEALTH.md`](docs/LINK-HEALTH.md)

## Contributing

Contributions are welcome; see [`CONTRIBUTING.md`](CONTRIBUTING.md). PRs require a one-time [Contributor License Agreement](CLA.md) (a license grant, **you keep ownership of your contributions**).

## License

quark's core is **AGPL-3.0-only**; see [`LICENSE`](LICENSE). Copyright © 2026 Lucas Olopes.

- **Self-hosting a single-account instance is free and unrestricted.**
- If you run a **modified** quark as a network service for others, the AGPL requires you to publish your modifications under the same license.
- The hosted, **multi-tenant cloud edition** (accounts, billing, tenant isolation) is a separate proprietary offering, not part of this AGPL core.
- **Commercial licenses** of the core (to use it without the AGPL's copyleft obligations) are available on request.
