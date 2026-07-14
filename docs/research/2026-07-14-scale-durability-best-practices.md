# Scale and durability: what production systems do

Date: 2026-07-14

A reference on how production systems solve the scaling and durability problems a
URL shortener with quark's feature set runs into. Each section gives the short
"what a production system does" summary, the failure mode of the naive in-process
approach, and the trade-offs, with sources as inline URLs. The point is to hold
this next to quark's in-binary implementations (in-memory webhook queue,
fire-and-forget analytics, per-process rate limiter, Feistel-permuted counter,
moka cache, LMDB/Postgres store) and see where an embedded shortcut stands in for
something a real dependency provides, and where it is genuinely fine.

This is not a verdict on quark. Several of quark's choices (Feistel codes, the
Postgres-shared scaling path, ClickHouse sink) already line up with the practices
below. The value here is a clean external baseline for the comparison.

---

## 1. Durable outbound webhook / event delivery

**What a production system does.** It writes the event row in the *same database
transaction* as the state change (the transactional OUTBOX pattern), so the event
either commits with the change or not at all, and a separate relay/worker polls
the outbox and publishes with at-least-once delivery plus an explicit ack before
the row is marked sent. This defeats the dual-write problem (updating the DB and
publishing to a broker as two separate operations, where a crash between them
loses or invents events). See https://microservices.io/patterns/data/transactional-outbox.html
and https://www.getconvoy.io/blog/webhooks-with-transactional-outbox .

**Failure mode of the naive approach.** An in-memory bounded queue (quark's
current webhook path) loses every un-delivered event on process restart, crash,
or deploy, and drops events silently when the queue is full. The subscription
config survives because it is persisted, but the delivery attempt does not. There
is no attempt log, no replay, and no "the receiver was down for an hour, catch it
up" story. quark's own WEBHOOKS.md is honest about this and names the durable
outbox as a future Postgres-gated enhancement.

**At-least-once vs exactly-once.** The outbox gives *at-least-once*, not
exactly-once. If the relay publishes an event but crashes before marking the row
delivered, it republishes on restart. Exactly-once across a network is not
achievable in general; the industry answer is at-least-once delivery plus
idempotent receivers. https://www.npiontko.pro/2025/05/19/outbox-pattern

**Idempotency / dedup.** Every event carries a stable unique id, and receivers
use it as an idempotency key to drop repeats. The Standard Webhooks guidance is
explicit: use the `webhook-id` header as an idempotency key, for example storing
seen ids in Redis for a few minutes so a redelivered webhook is processed once.
https://github.com/standard-webhooks/standard-webhooks/blob/main/spec/standard-webhooks.md
quark already emits a distinct `id` per event, which is exactly what a receiver
needs for this, so the producer side of dedup is in place; what is missing is the
producer-side retry/ack that makes redelivery happen at all.

**Retry, backoff, dead-letter.** Standard Webhooks recommends retrying on a
schedule that spans multiple days with exponential backoff plus random jitter
(jitter avoids synchronized retry storms when many deliveries fail on the same
downstream outage). After the retry budget is exhausted the message goes to a
dead-letter store, and producers expose a way to list failed messages and
manually replay them to recover from long outages. quark has the backoff-plus-
jitter part; it lacks the persisted attempt log, dead-letter, and replay.
https://github.com/standard-webhooks/standard-webhooks/blob/main/spec/standard-webhooks.md
, https://matheuspalma.com/blog/outbound-webhook-delivery-signing-retries-dead-letters

**Avoiding duplicate delivery across N worker replicas.** With more than one
relay running, two workers must not grab the same outbox row. The standard
Postgres technique is `SELECT ... FOR UPDATE SKIP LOCKED LIMIT n`: `FOR UPDATE`
locks the claimed rows, `SKIP LOCKED` makes a second worker step over rows another
worker already holds instead of blocking, so each worker leases a disjoint batch
and horizontal scaling is safe. https://www.glukhov.org/app-architecture/integration-patterns/transactional-outbox-pattern-go
The alternative is to hand the leasing to a real broker with consumer groups
(below) rather than reimplement it against the DB.

**Broker options and when each fits.**
- Redis Streams: sub-millisecond, consumer groups for at-least-once fan-out, good
  up to roughly 10K msg/s and an easy pick if Redis (or Valkey, which quark
  already speaks) is already in the stack. Retention is memory-bound.
- NATS JetStream: single-binary server that adds persistent streams, durable
  consumers, at-least-once, and replay, with much lower ops overhead than Kafka;
  the right middle ground for moderate throughput without standing up a cluster.
- Kafka: millions of msg/s, long retention, replay, partition-ordered log; the
  choice for large pipelines, event sourcing, and CDC, at the cost of partition
  planning and consumer-group tuning.
- Postgres-as-queue (the outbox table itself, drained with SKIP LOCKED): the
  lightest option, no new infrastructure, fine at modest rates. The known caveats
  are batching your reads (not `LIMIT 1`) and that the DB is doing broker work.
https://www.index.dev/skill-vs-skill/nats-vs-redis-vs-kafka ,
https://dev.to/young_gao/real-time-event-streaming-kafka-vs-redis-streams-vs-nats-in-2026-34o1
For quark, the proportionate step is the outbox drained with SKIP LOCKED (it
already has Postgres in shape 2), reaching for JetStream or Redis Streams only if
delivery volume or fan-out outgrows the DB.

---

## 2. High-throughput analytics / click ingestion

**What a production system does.** It does not update a shared aggregate row on
the hot path. It appends immutable event rows (or atomic counter increments) and
rolls them up asynchronously, or it ships events to a columnar store like
ClickHouse with batched, async inserts. Append-only writes avoid the row-level
lock contention that update-in-place creates, and a background process folds the
events into counters off the critical path.
https://medium.com/@timanovsky/ultra-fast-asynchronous-counters-in-postgres-44c5477303c3

**Failure mode of the naive approach.** Read-modify-write of an aggregate row
(`SELECT count; count = count + 1; UPDATE`) loses updates under concurrency: two
requests read the same value, both compute +1, and the second write silently
overwrites the first, so one click vanishes. Under load the aggregate row also
becomes a contention hotspot, generating write-write conflicts and rollbacks.
https://www.abstractalgorithms.dev/lost-update-database-anomaly ,
https://medium.com/double-pointer/transaction-for-system-design-interview-5-concurrent-writes-and-lost-updates-326f2abcc9f4
quark sidesteps this on the redirect path: clicks are captured fire-and-forget
into a tokio mpsc channel and folded by a background batch worker, so the 302
never does a read-modify-write and the redirect never waits on analytics. That is
the correct shape. The concurrency risk moves into the aggregation worker, which
must itself use atomic increments or single-writer folding rather than
read-modify-write.

**How ClickHouse ingestion is meant to work at scale.** Insert in batches, ideally
10,000 to 100,000 rows per insert, and keep insert *queries* to roughly one per
second so background merges keep up (too many small parts starve the merger). When
client-side batching is not feasible, enable async inserts (`async_insert = 1`),
which buffer rows server-side and flush on a size, time, or query-count threshold
(defaults around 100 MiB, 200 ms, or 450 queued queries). Keep the default
`wait_for_async_insert = 1` so the client is acked only after data is on disk,
which avoids silent loss if the server crashes before a flush.
https://clickhouse.com/docs/best-practices/selecting-an-insert-strategy ,
https://altinity.com/blog/using-async-inserts-for-peak-data-loading-rates-in-clickhouse

**Dedup and double counting across replicas.** ReplacingMergeTree deduplicates
rows sharing the ORDER BY key, but only on merge, which happens at
non-deterministic intervals, so it is best-effort and does not guarantee no
duplicates at query time. `FINAL` (at SELECT) or `OPTIMIZE FINAL` (forced merge)
give immediate dedup at a cost. ClickHouse also does automatic insert
deduplication for synchronous inserts (identical batches are safe to retry); for
async inserts this is off by default and must be enabled deliberately, and not
alongside dependent materialized views.
https://www.glassflow.dev/blog/replacingmergetree ,
https://clickhouse.com/blog/common-getting-started-issues-with-clickhouse
Multiple ingesting replicas avoid double counting either by attaching a stable
event id and leaning on insert-level dedup on retry, or by giving each event a
unique key so ReplacingMergeTree collapses accidental repeats. The general rule:
make the write idempotent, do not assume the merge already ran.

---

## 3. Distributed rate limiting

**What a production system does.** It keeps the counter in a shared store (Redis
or Valkey) so all replicas see one limit. The simplest form is an atomic
`INCR` on a per-key window with `EXPIRE` for cleanup; more accurate forms are a
token bucket (allows controlled bursts, refills at a steady rate) or a sliding
window counter (the common default for the best accuracy-to-cost ratio), both
usually implemented as an atomic Lua script so the check-and-decrement cannot
race. https://redis.io/docs/latest/develop/use-cases/rate-limiter/ ,
https://redis.io/tutorials/howtos/ratelimiting/

**Failure mode of the naive approach.** A per-process in-memory counter enforces
the limit *per replica*, so with N replicas behind a round-robin load balancer the
effective limit is N times the intended one. A "100 requests/min" rule with 5
replicas actually admits up to 500/min, and the number drifts as you autoscale.
quark's rate limiter already supports the shared Valkey backend keyed as
`quark:rl:{ip}:{window}`, which is the correct multi-node mode; the in-memory
variant is only correct for a single node.

**Trade-offs.**
- Latency: every rate-limit decision becomes a network round-trip to Redis. This
  is why the check is kept to one atomic op or one Lua script, not a read then a
  write.
- Fail-open vs fail-closed: if Redis is unreachable, fail-open (allow the request)
  keeps the limiter from becoming a single point of failure, and is the default
  for most external APIs, paired with circuit breakers and alerting. Sensitive or
  financial endpoints sometimes fail-closed (deny) to preserve the guarantee at
  the cost of availability. The choice is per-endpoint, not global.
https://oneuptime.com/blog/post/2026-01-21-redis-distributed-rate-limiter/view ,
https://dzone.com/articles/rate-limiting-strategies-redis
Since quark's rate limiter guards only `POST /` (create), never the redirect hot
path, fail-open there is the sensible default: an unreachable Valkey should not
block link creation.

---

## 4. Distributed ID generation for short, unguessable codes

**What a production system does.** Three families, each with a different
uniqueness-across-nodes story:
- Snowflake-style: a 64-bit id packed as timestamp + node/worker id + per-node
  sequence counter. Each node generates independently with no coordination and
  cannot collide with another node because the node-id bits differ; roughly 4,096
  ids per millisecond per node. Time-ordered but directly enumerable, so not
  unguessable on its own. https://www.algoroq.io/concepts/snowflake-id-vs-uuid/ ,
  https://www.systemdesignhandbook.com/guides/design-a-unique-id-generator-in-distributed-systems/
- DB sequences / hi-lo: a central atomic sequence (or hi-lo blocks handed out to
  each node to cut round-trips) guarantees global uniqueness through the DB.
  Simple and correct, but the DB is on the allocation path, and the raw ids are
  sequential and enumerable.
- Keyed format-preserving permutation of a counter (Hashids / Sqids / a Feistel
  network): take a plain counter and run it through a keyed bijection so the public
  code is non-sequential and hard to enumerate, while decode still maps back to the
  counter. Uniqueness is inherited from the counter (a bijection cannot map two
  ids to the same code), and non-enumerability comes from the key. Hashids/Sqids
  are the well-known off-the-shelf versions; note they are obfuscation, not
  encryption, so a determined attacker with enough samples can attack a weak key.
  https://sqids.org/faq
This is precisely quark's design: a counter (LMDB-persisted, or the atomic
`quark_id_seq` in Postgres) fed through a keyed ARX Feistel permutation and base62
-encoded. It gets uniqueness from the sequence, non-enumerability from the key, and
avoids any "does this code exist?" collision check because the permutation is a
bijection. The multi-node uniqueness question reduces to the counter: Postgres's
cluster-wide sequence handles it in shape 2, and the `QUARK_NODE_ID` bit-partition
(top 8 bits = node, low 32 = per-node counter) is the Snowflake-style answer for
the LMDB-per-node shape, at the cost of capacity per node.

**Failure mode of the naive approach.** A per-process in-memory counter with no
partitioning produces the same ids on two nodes and collides immediately. A random
code with a "check the DB, retry on collision" loop works but adds a read per
create and degrades as the keyspace fills. The permutation approach removes the
collision check entirely, which is why it is attractive here.

---

## 5. Cache coherence across replicas

**What a production system does.** It accepts a bounded staleness window and
picks an invalidation strategy to match. TTL is the floor: it caps how long stale
data can live but does nothing in the moment right after a write on another node.
For tighter coherence, an explicit invalidation or a pub/sub message (Redis
keyspace notifications, or a plain pub/sub channel) tells other replicas to drop
the key when data changes. Production systems combine them: short TTL as a safety
net, pub/sub for prompt invalidation. https://oneuptime.com/blog/post/2026-01-25-redis-cache-invalidation/view ,
https://redis.io/docs/latest/develop/pubsub/keyspace-notifications/

**Failure mode of the naive approach.** With a per-replica in-memory cache (moka,
in quark's case), a write served by node A updates A's cache and the shared store,
but nodes B and C keep serving the old value from their own caches until their TTL
expires. No node is wrong locally; the cluster is just inconsistent for the TTL
window. This is fine for a redirect target that rarely changes and tolerable
staleness, and painful for something that must reflect a write immediately.

**Trade-offs and what a redirect service should accept.** Pub/sub invalidation is
not free or perfectly reliable: Redis pub/sub is fire-and-forget, so a replica
that disconnects misses invalidations sent while it was gone, and in Redis Cluster
keyspace events are node-local and not broadcast, so a subscriber must listen on
every node. That means pub/sub tightens the window but does not close it, and a
short TTL still has to backstop it. For a URL shortener the pragmatic stance is to
treat redirects as eventually consistent: a code's destination is effectively
immutable in the common case, so a few seconds of staleness after an edit or
delete is acceptable, and a bounded TTL (optionally plus a delete-invalidation
signal) is enough. The one case that deserves prompt invalidation is
delete/expiry, where continuing to redirect a killed link for the full TTL is more
visible than a stale edit.

---

## 6. Single-binary vs necessary-dependency

**What a production system does.** It embeds when the embedded component gives a
*real* local guarantee (an on-disk B-tree, a WAL, a local index) and reaches for a
dependency when the job is fundamentally about coordination or durability *across
processes* that a single process cannot provide by itself. SQLite and LMDB are the
canonical "correct to embed" cases: they are durable local stores, not stand-ins
for a distributed system. The "just use Postgres" school pushes this further,
arguing that one Postgres instance can legitimately serve as your queue, outbox,
and job store for a long time before a dedicated broker earns its keep, because the
DB's transactionality is exactly what makes the outbox correct in the first place.
https://worlds-slowest.dev/posts/postgresql-message-queue/ ,
https://medium.com/@zhalokrahman007/outbox-pattern-database-as-message-broker-cdf4b788f78d

**Where it becomes an anti-pattern.** When an in-process component *fakes* a
guarantee that only a shared, durable, or coordinating dependency can actually
provide. An in-memory queue that presents itself as event delivery is the classic
example: it looks like a queue but loses everything on restart and coordinates
nothing across replicas, so it is durability theater. The tell is the mismatch
between the promise (events are delivered) and the substrate (RAM in one process).
The honest version of embedding is quark's webhook doc stating plainly that
delivery is best-effort and non-durable; the anti-pattern is when that caveat is
missing and callers assume a guarantee that is not there. The outbox-vs-broker
debates land on the same line: the DB-as-queue is a legitimate hack precisely
because the DB is durable and transactional, and it stops being legitimate the
moment you need cross-node fan-out, high throughput, or long retention that the DB
is not built to give. https://lobste.rs/s/4tlumh/how_implement_outbox_pattern_go_postgres

**Applied to quark.** The embedded stores (LMDB, moka, the in-proc analytics
channel) are the correct-to-embed kind: local, durable-or-explicitly-ephemeral, no
false cross-process promise. The in-memory webhook queue is the one that sits on
the anti-pattern line: it is embedding coordination/durability that only a shared
store or broker provides, and its correctness rests entirely on the doc being
read. Moving webhook delivery to the outbox pattern (drained from the Postgres it
already has, with SKIP LOCKED for multiple relays) converts that from theater into
a real guarantee without adding a new dependency, which is the proportionate move.

---

## Summary table

| Problem | Naive in-process failure | Production practice |
|---|---|---|
| Webhook delivery | In-memory queue loses events on restart; no replay | Transactional outbox + relay, at-least-once, idempotency keys, backoff+jitter, dead-letter, SKIP LOCKED for N relays |
| Click ingestion | Read-modify-write aggregate loses updates, hot-row contention | Append-only events + async rollup, atomic increments, or ClickHouse batched/async inserts with idempotent dedup |
| Rate limiting | Per-process counter gives N x limit with N replicas | Shared Redis/Valkey atomic counter (INCR+EXPIRE, token/leaky bucket, sliding window); fail-open by default |
| ID generation | In-memory counter collides across nodes | Snowflake (node-partitioned), DB sequence/hi-lo, or keyed permutation (Hashids/Sqids/Feistel) for non-enumerable, collision-free codes |
| Cache coherence | Per-replica cache serves stale until TTL | Bounded staleness: short TTL + explicit/pub-sub invalidation; redirects accept an eventual-consistency window |
| Embed vs depend | In-proc component fakes a cross-process guarantee | Embed for local durability (SQLite/LMDB); depend when the job is cross-process coordination or durable delivery |
