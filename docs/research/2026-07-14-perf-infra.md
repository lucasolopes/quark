**English**

# Deployment infrastructure for minimum redirect latency (no VPN)

This is a decision-support document. The question: what should quark run on to
serve the `/:code` 302 as fast as possible, globally, after dropping the VPN
layer the owner was fronting the box with. It grounds every recommendation in
quark's real architecture (see [ARCHITECTURE](../ARCHITECTURE.md) and
[SCALING](../SCALING.md)) and in current provider capabilities, cited inline.

The short version is at the bottom, in [Recommendation](#5-recommendation-three-tiers).

## 1. Why latency matters and where it comes from

quark's product KPI is redirect latency. The redirect is the whole product: a
click on a short link has to turn into a 302 to the destination before the user
notices a stall. Every other path (create, admin, analytics) is off the hot
path and can be slower without anyone caring. So the design target is the time
between the browser opening the connection and the 302 landing back.

The redirect budget breaks into two very different kinds of cost. The first is
network round trips, which are set by physics and geography and dwarf everything
else. The second is the work quark does once the request arrives, which is tiny.

Network cost, per new HTTPS connection to the origin:

- DNS resolution. A cold lookup on a fast anycast resolver is single-digit
  milliseconds; a slow or distant resolver is tens of ms. Anycast announces the
  same IP from many locations and routing sends the client to the nearest one
  (https://cleanbrowsing.org/learn/what-is-dns-latency). This is usually
  amortized by the client's DNS cache, so it hits mostly on the first click.
- TCP + TLS handshake. TLS 1.3 adds one round trip on a fresh connection;
  older setups take two, and combined with TCP setup a cold HTTPS connection
  can cost well over 500 ms before the first byte
  (https://www.thousandeyes.com/blog/optimizing-web-performance-tls-1-3). The
  number of round trips is fixed by the protocol; the length of each round trip
  is set by distance.
- Geographic distance to the origin. This is the dominant term and it is not
  negotiable by software. Light in fiber travels at about 2/3 of c, so roughly
  5 ms per 1000 km one way, and real routes run 1.5x to 2.5x the straight-line
  floor because of routing, hops, and metro fiber
  (https://geocables.com/internet-latency). New York to London is around 80 to
  120 ms RTT in practice; New York to Sydney is 200 to 300 ms RTT
  (https://hpbn.co/primer-on-latency-and-bandwidth/). A single TLS handshake
  plus the request is two to three of those round trips. A user in Singapore
  hitting a box in Germany pays this on every cold connection, and no amount of
  server tuning buys it back.

Server-side cost, once the request has arrived:

- The redirect compute. quark parses the base62 path segment and runs it
  through `permute::decode`, four rounds of ARX integer math with no I/O
  (ARCHITECTURE, "The Feistel/ARX permutation"). This is a handful of CPU
  cycles, sub-microsecond.
- The cache read. On an L1 hit, moka returns an already-materialized `Record`
  with no store transaction and no JSON parse (ARCHITECTURE, "Pluggable
  backends"). This is nanoseconds to low microseconds. On an L1 miss it falls to
  the L2 (Valkey, wrapped in a 100 ms timeout and a circuit breaker) or the
  store (an LMDB mmap page-table lookup, or a Postgres query). There is no
  `code -> id` storage lookup at all; the id is computed, not stored.
- Proxy and load-balancer hops. Each in-path proxy (the platform edge, a
  reverse proxy, a load balancer) adds a small hop. Within one datacenter these
  are sub-millisecond, but a hop that sits in a different location than the
  origin adds a full extra RTT to that location.
- A VPN or tunnel hop. If the redirect traffic is routed through a VPN or a
  tunnel before reaching quark, that adds at least one extra network leg, often
  to a concentrator in a third location, plus per-packet encryption overhead.
  This is the layer the owner wants gone, and for the hot path it is pure added
  latency (more in [section 3](#3-dropping-the-vpn)).

The takeaway sets up everything below: quark's own work is already close to the
floor (sub-millisecond decode plus a memory-cache read), so the redirect budget
is almost entirely network round trips. You cannot make quark meaningfully
faster by optimizing the binary. You make it faster by putting a copy of the
answer closer to the clicker, and by removing hops from the path. That is an
infrastructure problem, not a code problem.

## 2. The geo-distribution problem

A URL shortener is read-heavy and global. quark's own workload assumption is
roughly 200:1 read:write (ARCHITECTURE, "Why these choices"). Clicks come from
everywhere, and each one wants the nearest server. A single box in one region
serves its own continent well and every other continent badly, because the
distance term from [section 1](#1-why-latency-matters-and-where-it-comes-from)
is fixed. The lever is to cut the round-trip distance between the clicker and
whatever answers the 302. There are four ways to do that, and they are not
mutually exclusive:

- Anycast so the client reaches the nearest point of presence by routing.
- Multi-region deployment so an actual quark instance runs near the client.
- Edge compute so the redirect logic runs in a CDN's runtime at hundreds of
  locations.
- A CDN in front that caches the 302 response itself, so repeat clicks on a hot
  link never reach the origin.

The last one deserves a note up front, because it is the cheapest big win and it
composes with all the others. quark already sets a TTL-aware `Cache-Control` on
the redirect (ARCHITECTURE, "Redirect flow"). A 302 for a public short link is
cacheable. Put a CDN in front, and a hot link's redirect is served from the CDN
edge near the user after the first miss, at edge RTT, without the origin being
touched at all. This does not help the long tail of cold links, and it needs
care so that an edited or deleted link is not served stale past its TTL, but for
the head of the click distribution it turns a cross-ocean origin trip into a
nearby edge hit. Cloudflare runs 330+ points of presence versus Fastly's ~100
(https://blog.blazingcdn.com/en-us/fastly-vs-cloudflare-which-cdn-is-faster-for-global-delivery).

Now the deployment shapes, compared for quark specifically.

### Single bare-metal box

One strong dedicated server in the primary region. Bare metal has no
hypervisor and no noisy-neighbor contention, so per-request timing is the most
consistent you can get and the CPU is entirely yours
(https://www.cherryservers.com/blog/bare-metal-vs-cloud). For quark this fits
perfectly: it runs the real single binary with LMDB or Postgres locally, the
store and cache sit on the same machine or the same LAN, and the redirect path
has zero cross-datacenter hops. This is the fastest option for users in that one
region and the cheapest per unit of compute. Hetzner's dedicated servers are
strong value but Europe-only for bare metal
(https://gartsolutions.com/ovh-vs-hetzner/); OVHcloud has wider geographic
reach if North America matters
(https://www.ovhcloud.com/en/learn/dedicated-server-vs-bare-metal/). The
trade-off is the obvious one: it is single-region. A clicker on another
continent pays the full distance every time, unless a CDN in front is absorbing
the hot links.

### Multi-region VMs

The same binary, deployed as several instances across regions, behind
geo-routing. This is quark's SCALING shape 2 (replicas on shared Postgres plus
Valkey) stretched across regions. It runs the real binary near users on several
continents, so cold clicks land on a nearby instance. The cost is operational:
you now run N instances, the data layer has to answer reads locally in each
region (covered in [section 4](#4-the-data-layer-under-multi-region)), and you
own the orchestration. A plain multi-region VM setup on a hyperscaler works but
you assemble the geo-routing and the private networking yourself.

### Fly.io

Fly.io is the multi-region VM shape with the assembly already done, and it is
the closest fit to quark's topology. It runs your actual container (the real
Rust binary, unmodified) in many regions, gives every app a shared anycast IPv4
and unlimited anycast IPv6 for global load balancing
(https://fly.io/docs/networking/services/), and routes each request to the
nearest region where you have an instance. There is no runtime rewrite: quark
runs as-is. It has a documented pattern for exactly quark's read-heavy shape,
read from a regional Postgres replica locally and use the `fly-replay` header to
forward the rare write to the primary region
(https://fly.io/docs/blueprints/multi-region-fly-replay/). Pricing is
pay-as-you-go per machine per region, a shared anycast IPv4 included and
dedicated IPv4 at $2/mo, outbound data from $0.02/GB in North America and Europe
(https://fly.io/docs/about/pricing/). For quark this is the pragmatic path to
"real binary near users" without building the platform yourself. The trade-off
is that you are on their platform and their per-region machine and bandwidth
costs, and you still run and pay for the data layer.

### Cloudflare Workers and Fastly Compute

These are edge runtimes: your code runs in a V8 isolate (Workers) or a Wasm
sandbox (Fastly) at every point of presence, with cold starts of 2 to 5 ms on
Workers and under 500 microseconds on Fastly
(https://blog.easecloud.io/cloud-infrastructure/edge-computing-for-saas-performance/).
Geographically this is the best possible answer to the distance problem, because
the logic runs within tens of milliseconds of almost every user on Earth.

The catch for quark is that it is not the real binary. quark is an axum server
that owns a TCP listener, opens LMDB via mmap, and holds pooled connections to
Postgres and Valkey. None of that runs in an edge isolate. You would rewrite the
redirect path against the platform's runtime and storage primitives. The pure
functions port cleanly (the `permute` and `codec` math is just integer
arithmetic, and Rust compiles to Wasm), but the store and cache do not. You
cannot mmap LMDB at the edge, and a raw pooled Postgres or Valkey connection
from an isolate is not the native model. Workers can reach Postgres over TCP via
`tokio-postgres` on a Workers socket, usually fronted by Hyperdrive for pooling
and query caching
(https://developers.cloudflare.com/workers/languages/rust/crates/), but that is
a different data path than quark's, and a cross-region connection from the edge
to a single Postgres reintroduces the distance you were trying to remove. The
honest framing: the edge is a rewrite of the read path onto the platform's own
KV or D1 store, keyed by the code, with the origin as the source of truth. That
is a real and fast design, but it is a second implementation of quark's hot
path, not a deployment of the existing one. Workers pricing starts at $5/mo for
10M requests and 30M CPU-ms, then $0.30/M requests
(https://developers.cloudflare.com/workers/platform/pricing/).

### AWS Lambda@Edge and CloudFront Functions

CloudFront Functions run at the edge with a sub-millisecond budget but have no
network access at all, cannot read a request body, and cap at 10 KB of compiled
code with a roughly 1 ms execution limit
(https://docs.aws.amazon.com/AmazonCloudFront/latest/DeveloperGuide/edge-functions-restrictions.html).
They are built for header rewrites and static redirects, so they can do the
`permute::decode` math but cannot look a destination up in any store. Useful
only if the mapping is small enough to bake into the function or the CDN cache,
which quark's is not in general. Lambda@Edge can make outbound network calls and
has real memory and time budgets, but it runs at the regional edge, not the
outer edge, its cold starts are the classic Lambda cold starts, and reaching
back to a single origin database from it puts the distance right back in the
path
(https://docs.aws.amazon.com/AmazonCloudFront/latest/DeveloperGuide/edge-functions-choosing.html).
For quark, CloudFront Functions plus a cached origin is a viable "CDN in front"
variant, but Lambda@Edge as the redirect engine is a worse fit than either
Fly.io or a Workers rewrite.

### Deno Deploy

Deno Deploy is another V8-isolate edge platform, with roughly 3 ms cold start
and low double-digit P50
(https://blog.easecloud.io/cloud-infrastructure/edge-computing-for-saas-performance/).
Same structural mismatch as Workers for quark: it runs JavaScript and Wasm at
the edge, not the axum binary, so it is a rewrite, not a deployment. No
particular advantage over Workers for this workload, and a smaller data-store
ecosystem to lean on.

### Summary of the shapes

| Shape | Runs quark's real binary? | Latency it buys | Main trade-off |
|---|---|---|---|
| Single bare-metal box | Yes | Best in one region, worst elsewhere | Single region; other continents pay full distance |
| Multi-region VMs (self-run) | Yes | Near-user on several continents | You build geo-routing and private networking |
| Fly.io | Yes | Near-user, anycast, `fly-replay` for writes | Platform lock-in; you still run the data layer |
| Cloudflare Workers / Fastly Compute | No (rewrite) | Best global, 2-5 ms / sub-ms cold start | Rewrite of the read path onto their runtime and store |
| CloudFront Functions | No (no network) | Sub-ms, but cannot reach a store | Only static/computed redirects; no lookup |
| Lambda@Edge | No (rewrite) | Regional edge, real cold starts | Origin DB round trip reintroduces distance |
| Deno Deploy | No (rewrite) | ~3 ms cold start | Rewrite; weaker data-store ecosystem than Workers |

## 3. Dropping the VPN

The owner was fronting quark with a VPN or tunnel and wants it gone from the hot
path. Worth being precise about what that layer was doing, so nothing important
is dropped with it.

A VPN or tunnel in front of a single origin box usually buys two things. First,
origin hiding: the real server IP is not exposed, so it cannot be hit directly
or trivially DDoSed, and inbound traffic is forced through the tunnel. Second,
private networking: the box reaches its database and cache over an encrypted
private link rather than the public internet, and admin access (SSH) is not open
to the world.

Both are real needs. The mistake is paying for them with a hop on the redirect
hot path. The fix is to get the same two properties without routing the 302
through a tunnel.

For origin hiding, put a CDN or reverse proxy in front (Cloudflare is the
obvious pick since you likely want its anycast DNS and caching anyway) and
firewall the origin so it only accepts traffic from that front. The public
sees the CDN's anycast IP; the origin IP is not advertised. This replaces the
VPN's hiding function with a layer that also caches redirects and terminates TLS
near the user, so it is a latency win, not just a wash. quark already reads the
client IP from a configurable header (`QUARK_REAL_IP_HEADER`, default
`cf-connecting-ip`) with a socket fallback, and its geo rules read
`cf-ipcountry` (ARCHITECTURE, "Abuse protection" and "Destination precedence"),
so it is already built to sit behind Cloudflare.

For private DB and cache networking, keep it private without a tunnel on the hot
path by co-locating and using provider-native private networking:

- Put quark and its Postgres and Valkey in the same region on the same private
  network, so the store reads never traverse the public internet and never touch
  a VPN concentrator. On Fly.io this is 6PN, a WireGuard mesh that is automatic
  and default-locked to your organization
  (https://fly.io/blog/incoming-6pn-private-networks/). On Hetzner it is vSwitch
  or private networks joining cloud and dedicated servers over a private link
  (https://docs.hetzner.com/cloud/networks/connect-dedi-vswitch/). On a
  hyperscaler it is the VPC. In every case the database is not on a public IP.
- Require TLS (ideally mutual TLS) on the Postgres and Valkey connections, so
  even on the private network the link is authenticated and encrypted. This
  gives you the VPN's confidentiality property on the data link without a
  separate tunnel process in the path.
- Firewall the database to accept connections only from quark's private
  address, and keep admin access (SSH) behind a bastion or an on-demand access
  tool rather than open to the internet
  (https://community.hetzner.com/tutorials/vpc-with-wireguard-pfsense/). Admin
  access is off the hot path, so a bastion or a WireGuard jump host there costs
  nothing in redirect latency.

The distinction that makes this safe: the VPN was on the redirect path, and
that is what has to go. Private networking, mTLS, and a firewall are not a hop on
the 302; they are properties of a link that is already inside one region. You
keep origin hiding (CDN plus firewall) and private DB access (VPC or 6PN plus
mTLS) while the redirect itself goes client, CDN, origin, done, with no tunnel
leg.

## 4. The data layer under multi-region

Once quark runs in more than one region, the redirect read must be answerable
locally in each region, or you have moved the binary near the user but left the
data far away, which buys nothing. quark's design already handles most of this;
the multi-region data layer is mostly about where the reads land.

The read path. The redirect needs one thing from storage: the `Record` for an
id, and only on a cache miss. Per-region, that means a warm cache plus a local
read source:

- Valkey near each node. quark's L2 is a Valkey tier with a 3600s TTL, consulted
  only on an L1 miss, wrapped in a 100 ms timeout and a circuit breaker so a slow
  or distant Valkey never stalls a redirect (ARCHITECTURE, "Pluggable
  backends"). Run a Valkey in each region so the L2 hit is local. Because the L1
  moka map (60s TTL) sits in front of it, most reads never even reach Valkey.
- Postgres read replicas per region for the cache-miss tail. A regional read
  replica lets the local quark answer a cold read without crossing an ocean to
  the primary. Fly.io documents exactly this: read from the regional replica,
  and forward the rare write to the primary with `fly-replay`
  (https://fly.io/docs/blueprints/multi-region-fly-replay/). Managed Postgres
  with multi-region read replicas is available from Supabase and from Neon on
  their scale tiers, with the caveat that a user far from the primary sees
  100 to 150 ms on cross-region reads if there is no local replica
  (https://supabase.com/features/read-replicas). The replica exists precisely to
  remove that.

The write path. Writes (create, edit, delete, block) are rare in a 200:1
workload and are not on the redirect hot path, so they can pay to reach the
primary region. Single-leader Postgres with regional read replicas is the
realistic architecture: one primary takes writes, every region reads its local
replica. quark's create path already asks the store for an atomic id
(the shared `quark_id_seq` on Postgres) and does no `code -> id` lookup, so the
write is a single insert to the primary (ARCHITECTURE, "Create flow"). Routing
the write to the primary is either `fly-replay` (Fly.io) or writing to the
primary connection string directly from any region.

Cross-node consistency and the acceptable window. quark already ships the piece
that makes multi-region reads correct fast enough: Valkey pub/sub invalidation
on the `quark:invalidate` channel (SCALING, "Cross-node consistency windows",
and `src/invalidate.rs`). An admin edit or delete publishes `link:<id>`, and
every node, in every region subscribed to that Valkey, drops the matching L1
entry on receipt. The per-node TTL (60s) is the backstop if a message is missed.
The publish is bounded by a 100 ms timeout and is fail-open, so a slow Valkey
never blocks the admin write. The one added multi-region wrinkle is Postgres
replica lag: a freshly created link may take a replication moment to appear on a
distant replica, and a freshly edited link may serve the old destination until
the invalidation lands and the replica catches up.

For a URL shortener this eventual-consistency window is acceptable, and quark's
own docs already take that position: click ingestion is at-most-once by design,
and cache and blocklist propagation are eventually consistent bounded by the
pub/sub channel with a TTL backstop (SCALING, "Analytics ingestion is
at-most-once" and "Cross-node consistency windows"). A newly created link being
invisible in a far region for a replication moment, or an edited link serving its
old target for a second or two, does not break anything a shortener promises. The
one case to handle deliberately is a blocked destination: a blocklist change
rides the same pub/sub channel and should propagate on it, not wait on the TTL,
which is already how quark works. The realistic architecture, then, is single
primary Postgres, one read replica per region, one Valkey per region, quark's
existing pub/sub invalidation across all of them, and writes routed to the
primary. The redirect reads local, always.

## 5. Recommendation: three tiers

Pick by how global the audience actually is and how much operational surface you
want to own. All three drop the VPN from the hot path and keep the database
private by co-location plus mTLS plus a firewall, per
[section 3](#3-dropping-the-vpn).

### Tier 1: one strong box, CDN in front (start here)

One bare-metal server or a large VPS in your primary region, running the real
quark binary with Postgres and Valkey on the same private network (no VPN, mTLS
on the DB link, firewalled to the origin). Cloudflare in front for anycast DNS,
TLS termination near the user, and caching of the hot-link 302s. The origin only
accepts traffic from Cloudflare, which replaces the VPN's origin-hiding for
free.

- Latency: best-in-region redirects (no cross-datacenter hop, quark's
  sub-millisecond decode plus a local cache read), and near-edge redirects for
  hot links worldwide because the CDN serves the cached 302 from a nearby point
  of presence. The cold long tail from distant regions still pays the distance
  to the origin.
- Cost and complexity: lowest of the three. One box, one CDN, no cross-region
  data layer. This is a small change from today's single-VPS Coolify deploy in
  [DEPLOY](../DEPLOY.md): drop the VPN, add the CDN and the firewall rule.
- Pick it if the audience is concentrated on one continent, or to ship the
  latency win now and add regions later.

### Tier 2: multi-region Fly.io running the real binary (the performance pick)

quark deployed on Fly.io in the regions your clicks come from, the actual
binary, no rewrite. Anycast routes each click to the nearest region
(https://fly.io/docs/networking/services/). Each region has a local Valkey and a
local Postgres read replica; a single primary Postgres takes writes, forwarded
with `fly-replay`
(https://fly.io/docs/blueprints/multi-region-fly-replay/). quark's pub/sub
invalidation keeps every region's cache correct within the bounded window from
[section 4](#4-the-data-layer-under-multi-region). Cloudflare can still sit in
front for DNS and hot-link caching. Private networking is Fly's 6PN, so the DB
link is private with no VPN
(https://fly.io/blog/incoming-6pn-private-networks/).

- Latency: near-user on every continent you deploy to, cold clicks included,
  because a real quark with a local replica answers them. This is the strongest
  "max performance, min latency" option that still runs the code you have.
- Cost and complexity: higher. You pay per machine per region and for
  cross-region bandwidth (https://fly.io/docs/about/pricing/), and you operate a
  primary-plus-replicas Postgres and a per-region Valkey. But there is no second
  implementation of the hot path to maintain.
- Pick it when one region is no longer enough and the click distribution is
  genuinely global. This is the recommended target for "maximum performance"
  because it buys near-user latency worldwide without forking the codebase.

### Tier 3: edge-runtime rewrite (only if global sub-50ms is mandatory)

Rewrite the redirect read path onto Cloudflare Workers or Fastly Compute, so the
`permute::decode` plus a keyed lookup runs in an isolate at 300+ points of
presence, backed by the platform's own KV or D1 store with the origin as source
of truth (https://developers.cloudflare.com/workers/platform/pricing/). Cold
starts are 2 to 5 ms on Workers and under 500 microseconds on Fastly
(https://blog.easecloud.io/cloud-infrastructure/edge-computing-for-saas-performance/).

- Latency: the best physically possible, tens of milliseconds from almost any
  user, because the logic runs at the outer edge and there is no origin trip on
  a hit.
- Cost and complexity: highest. This is a second implementation of quark's hot
  path against a different runtime and a different store, kept in sync with the
  origin. You now maintain two redirect engines and the replication between
  them. quark's pure core (`permute`, `codec`) ports cleanly to Wasm, but the
  store, cache, cluster, and analytics layers do not.
- Pick it only if a hard global sub-50ms redirect SLA is a product requirement
  that Tier 2 cannot meet. For most shorteners it is not, and the CDN caching in
  Tier 1 and Tier 2 already delivers edge latency on the hot links that make up
  most clicks.

### What to pick

Pragmatically, for "max performance, min latency" without over-building:

- Ship Tier 1 now. It removes the VPN, keeps the DB private, and adds a CDN that
  gives most of the global latency benefit (cached hot-link 302s at the edge) at
  the lowest cost and complexity. It is a small delta from the current deploy.
- Move to Tier 2 when the audience is provably multi-continental and the cold
  long tail from distant regions is a real complaint. Fly.io running the real
  binary with regional replicas is the performance sweet spot, because it buys
  near-user latency everywhere without a rewrite.
- Reserve Tier 3 for a mandated global sub-50ms SLA. It is the fastest and the
  most expensive to build and operate, and quark's architecture (LMDB mmap,
  pooled Postgres and Valkey, axum) does not lift to an isolate without a
  second implementation of the read path.

The through-line: quark's own redirect work is already at the floor, so latency
is bought with placement (a copy of the answer near the clicker) and by removing
hops (the VPN goes, a CDN and private DB networking take its place). Tier 1 gets
most of it cheaply; Tier 2 gets the rest without forking the code; Tier 3 is the
edge-latency ceiling at the cost of a second codebase.

## Decisão do dono (2026-07-14)

Prioridade declarada: **menor latência e máxima disponibilidade** (custo e "começar
barato" não são a trava). Isso aponta para o **Tier 2 (multi-região no Fly.io
rodando o binário real)** como alvo, porque é o único que entrega os dois ao mesmo
tempo: réplicas regionais colocam uma cópia perto de cada clicador (latência
mínima) e a redundância entre regiões dá disponibilidade máxima, tudo sem reescrever
o read-path. Os blocos do Tier 1 (tirar a VPN, DB privado, CDN na frente cacheando
os 302 quentes) são adotados no caminho, não como destino final. O Tier 3
(reescrita edge-runtime) fica fora a menos que apareça um SLA global de sub-50ms
obrigatório, pelo custo de um segundo código do read-path. Ponto de atenção para a
disponibilidade: a camada de dados sob multi-região (seção 4) precisa acompanhar
(réplicas de leitura regionais / Postgres com failover), senão o banco vira o ponto
único que derruba a meta.
