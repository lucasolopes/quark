**English** · [Português](EDGE.PT_BR.md)

# Edge / CDN and redirect caching

## Why edge would help

quark resolves a redirect in ~2 ms — the measured bottleneck has never been
the server, it's **geography**: every `GET /:code` makes a round trip (RTT) to
the single instance, which lives in one region only. A user on the other side
of the world pays that full RTT on every click, even though the link never
changes.

## What quark already sends (and it already works)

Every redirect response carries a `Cache-Control` header computed from the
link's TTL (`src/api.rs`, `cache_control_for`):

| Situation | Status | `Cache-Control` |
|---|---|---|
| Link without TTL | 302 | `public, max-age=86400` (1 day) |
| Link with TTL, still alive | 302 | `public, max-age=<seconds until expiry>` (never > 86400) |
| Nonexistent code/alias | 404 | `no-store` |
| Expired link | 410 | `no-store` |

**Browsers respect this header.** When the same user clicks the same link
again, their browser serves the redirect **from the local cache, without
touching the network.** This gain is real and already active — it's
per-user, not per-region.

## Measured reality: Cloudflare does NOT cache the 302

> Tested on this deploy (Cloudflare, free plan, behind a Cloudflare Tunnel):
> even with a **Cache Rule** marking the path as *Eligible for cache* **and**
> a forced fixed **Edge TTL**, `Cf-Cache-Status` stayed **`DYNAMIC`** on every
> request. Cloudflare treats **3xx redirects as dynamic** and never places
> them in the edge cache.

In other words: **creating a Cache Rule to cache the 302 doesn't help** —
it's not a misconfiguration, it's platform behavior. Don't spend time on it.

## With Cloudflare Tunnel (Coolify's native option)

If you use Coolify's `cloudflared` (recommended), traffic **already passes
through Cloudflare's edge** by construction (confirmable via the `cf-ray`
header in the response), and **DNS + TLS are handled by the tunnel** —
no proxied A record, SSL mode, or origin certificate to configure. But, per
the item above, the edge still does **not** cache the 302.

## If you REALLY need a cached redirect at the edge

The only reliable path on Cloudflare is a **Worker**: an edge script that
either (a) performs the redirect itself, reading the code→URL pair from
**Workers KV**, or (b) caches the origin's 302 via the **Cache API**. That's
a genuine phase-2 change — it requires getting link data to wherever the
Worker can reach it (dual-write to KV, or the Worker querying the origin and
caching the result).

**Is it worth it?** Only if you have meaningful traffic **far** from the
VPS's region. For an audience close to the origin (e.g. a VPS in Europe with
European users), RTT is already low and the Worker doesn't pay off. Current
deliberate decision: **don't build it** — `Cache-Control` is already in
place for whenever/if it becomes worth it.

## Summary

- Correct `Cache-Control` on the 302 → **browser cache works** (per-user). ✓
- **Cloudflare edge cache for the 302** → **doesn't work** (3xx is dynamic). ✗
- Real edge for a dynamic redirect → **Worker** (phase 2, low ROI if the
  audience is close to the origin).
