**English** · [Português](SCALING.PT_BR.md)

# Horizontal scaling in quark

quark scales horizontally by **sharing storage** across replicas. There are
three deployment shapes, with different limits: pick the one that matches
what you need.

## The three shapes

```mermaid
flowchart TB
    subgraph F1["1. Pure binary (default)"]
        A1[single quark] --> L1[(local LMDB)]
    end
    subgraph F2["2. Replicas + shared Postgres (recommended for multi-node)"]
        LB[Load Balancer] --> R1[quark 1]
        LB --> R2[quark 2]
        LB --> R3[quark N]
        R1 --> PG[(shared Postgres)]
        R2 --> PG
        R3 --> PG
    end
    subgraph F3["3. Multiple LMDB binaries (not recommended for multi-node)"]
        N1[quark node 0] --> D1[(local LMDB 0)]
        N2[quark node 1] --> D2[(local LMDB 1)]
    end
```

| Shape | Storage | Multi-node | Note |
|---|---|---|---|
| **1. Pure binary** | Embedded LMDB | No (1 node) | Minimal footprint; ~1.1 trillion links of capacity |
| **2. Replicas + Postgres** | Shared Postgres | **Yes** | Recommended path; any replica serves any link |
| **3. Multiple LMDB** | Local LMDB per node | Not for reads | Each node only has the data it created (see limits below) |

## The honest scale matrix

Not every subsystem scales the same way. A "multi-node" deployment that only
shares the store but not Valkey is still degraded: rate limits become N-times
the configured value and cache/blocklist coordination lags. Here is what each
subsystem actually does per deployment shape.

| Subsystem | Single-node (LMDB) | Multi-node (Postgres + Valkey + ClickHouse) |
|---|---|---|
| Redirect (hot path) | fine, one node | computed code + cache tier, any replica serves any link |
| ID allocation | per-node counter + node_id prefix (needs a unique node_id) | shared `quark_id_seq`, coordinated across replicas |
| Rate limit | in-memory, per node (correct on one node) | atomic global counter in Valkey |
| Blocklist | per-node snapshot + TTL | shared snapshot + Valkey L2, invalidation via pub/sub |
| Cache | per-node L1 (correct: store is not shared) | per-node L1 + shared L2, invalidation via pub/sub |
| Analytics aggregation | per-node blob RMW (undercounts across nodes) | ClickHouse append-only + aggregate-on-read (Postgres is correct but a per-link hotspot) |

**Single-node (default):** LMDB with the in-memory cache and rate-limit is
correct and needs no external dependencies. This is the pure-binary shape.

**Multi-node:** requires Postgres (shared store) plus Valkey (shared rate-limit
and cross-node cache/blocklist invalidation) and ClickHouse is recommended for
analytics (append-only and scalable; the Postgres analytics path is correct but
becomes a per-link write hotspot).

## How to actually scale (shape 2)

Bring up N copies of the binary behind a load balancer, all with the same
`QUARK_KEY` and the same `QUARK_DATABASE_URL` pointing at the shared Postgres:

- **Unique ids**: Postgres's `quark_id_seq` sequence is atomic and cluster-wide;
  concurrent replicas never generate the same id. The permute width is 40 bits,
  so the global ceiling is 2^40 links (about 1.1 trillion) across the whole
  cluster.
- **Shared data**: every replica reads/writes the same tables; there's no
  session affinity needed (the load balancer can be plain round-robin).
- **Shared rate-limit and invalidation**: point every replica at the same
  Valkey (`QUARK_VALKEY_URL`). Without it, each replica keeps its own in-memory
  counter and the effective rate limit becomes N-times the configured value,
  and cache/blocklist changes only propagate on the per-node TTL.
- **Fail fast if you meant to cluster**: set `QUARK_STRICT_CLUSTER=1` on every
  replica and quark refuses to start unless BOTH `QUARK_DATABASE_URL` and
  `QUARK_VALKEY_URL` are present. Any non-empty value turns it on. This turns a
  silent misconfiguration (N-times rate limits, stale caches, per-node LMDB
  files) into a startup error. Single-node deployments leave it unset and are
  unaffected.

## Cross-node consistency windows

Two subsystems are eventually consistent between replicas, both bounded and
both closed by the Valkey pub/sub invalidation channel:

- **Cache** (`patch`/`delete`): without pub/sub, another replica's L1 can serve
  a stale link until its per-node TTL expires (default 60s). The invalidation
  channel publishes on every mutation and each replica clears its L1 on receipt,
  so the window drops from up to 60s to near-instant. The TTL stays as the
  backstop if a replica misses a message.
- **Blocklist**: without pub/sub, a newly blocked entry propagates on the
  snapshot TTL (`QUARK_BLOCKLIST_TTL`, default 60s). The same channel makes it
  near-instant, with the TTL as the backstop.

## `QUARK_NODE_ID`: defensive LMDB partitioning

quark's code space is 40 bits. When `QUARK_NODE_ID` is **set** (0–255), the
top 8 bits identify the node and the low 32 bits become that node's local
counter:

| Node bits | Local bits | Max nodes | Links per node |
|---|---|---|---|
| 8 | 32 | 256 | ~4.3 billion |

- **Unset (default)**: normal behavior, the counter uses the full 40 bits
  (~1.1 trillion links). This is single-node mode.
- **All-or-nothing rule**: either **every** node runs without `QUARK_NODE_ID`
  (= 1 node), or **every** node runs with a **distinct** `QUARK_NODE_ID`.
  Never mix an un-partitioned node (full range) with partitioned ones: the
  spaces overlap.
- **Uniqueness is on you**: the id MUST be unique per replica (a StatefulSet
  ordinal is a natural source). quark cannot detect a duplicate; two nodes with
  the same id silently reuse the same code space and collide.
- An invalid `QUARK_NODE_ID` (outside 0–255) crashes the process at startup.
- `QUARK_NODE_ID` is **LMDB-only**. On the Postgres backend it is ignored (the
  shared sequence handles allocation), and quark logs that it was ignored. The
  Postgres path has a single global ceiling of 2^40 links, not a per-node one.

## The honest limit of shape 3

`QUARK_NODE_ID` guarantees that two LMDB nodes **won't generate the same
code**, but it does **not** make one node serve another node's links. Each
LMDB is local: a redirect that lands on the wrong node returns 404, because
that node doesn't have the data. In other words, node-id is a
**collision guard-rail**, not a real multi-node mode.

**By design, a pure binary (LMDB, no database) is single-node**: this is a
deliberate constraint of the system, not a limitation to be removed. **For
multi-node, use shape 2 (shared Postgres).**
