**English** · [Português](ANALYTICS.PT_BR.md)

# Click analytics and privacy

This document explains what quark records when someone clicks a short link, and the privacy decisions behind it. If you're deciding whether to enable geo headers on your proxy, or you just want to know what data quark keeps about your visitors, this is the page.

## What gets captured on a click

Every redirect (`GET /:code`) builds one `ClickEvent` off the request headers, fire-and-forget: the redirect itself never waits on analytics. The event holds:

| Field | Source | Notes |
|---|---|---|
| `country` | `cf-ipcountry` header | Two-letter code from the edge proxy, not looked up by quark |
| `city` | `cf-ipcity` header | Optional; empty on most deploys (see below) |
| `referer` | `Referer` header | Full value kept in the recent-events ring; aggregates group by host only |
| `user_agent` | `User-Agent` header | Used to derive device, OS and browser; the raw string isn't exposed in aggregates |
| `ts` | server clock | Click timestamp |

From that event, quark computes:

- **Per-day** clicks, for the time series chart.
- **Per-country** and **per-city**, from the proxy's geo headers.
- **Per-device** (Mobile / Desktop / Other), **per-OS** (Windows / macOS / iOS / Android / Linux / Other) and **per-browser** (Chrome / Safari / Firefox / Edge / Other), all from heuristic parsing of the user agent string. No external UA database, no added dependency: same style as the existing `device_from_ua` parser.
- **Per-referrer**, grouped by hostname (`news.ycombinator.com`, `direct` when there's no referrer, `other` when the referrer doesn't parse as a URL). Grouping by host, not the full URL, keeps the breakdown from growing an unbounded number of buckets.

## Privacy posture

**quark never stores an IP address.** Not in the LMDB backend, not in ClickHouse, not in memory beyond the single request that's being handled. Country and city come from a header the edge proxy already computed (`cf-ipcountry`, `cf-ipcity`); quark reads that header and moves on. There's no GeoIP database, no IP-to-location lookup, no dependency that would need one.

What quark does keep:

- **Aggregates**: counters per day, country, city, device, OS, browser and referrer host. These are just numbers; they can't be traced back to an individual visit.
- **A capped ring of recent events**: the last N raw `ClickEvent` rows per link. The LMDB backend keeps at most `EVENTS_MAX` (1000) per link, dropping the oldest once that fills; the ClickHouse backend applies a `LIMIT` on the same query. This is what backs the "Recent events" table on the stats screen, and it holds the same fields as above, no IP among them.

If you don't send `cf-ipcity` (or don't run behind a proxy that sets it), `per_city` is simply empty, and the UI hides that chart instead of showing an empty one. Most self-hosted setups fall into this bucket: city is opt-in, not a default expectation.

## Turning on geo headers

quark reads two headers if they're present; it never looks them up itself:

- `cf-ipcountry`: set automatically by Cloudflare on every request that passes through its network (see [`docs/EDGE.md`](EDGE.md) for how quark sits behind Cloudflare). No configuration needed once you're behind Cloudflare.
- `cf-ipcity`: **not** set by default on the free Cloudflare plan. Enabling it requires a paid plan with the ["Add visitor location headers" managed transform](https://developers.cloudflare.com/rules/transform/managed-transforms/reference/#add-visitor-location-headers) turned on (Rules → Transform Rules → Managed Transforms, or the equivalent API call).

If you're behind a different proxy (nginx, Traefik, another CDN), set the equivalent headers yourself at the edge and quark will pick them up the same way, since the header names are the only thing it depends on. There's no vendor lock-in in the analytics code: any proxy that sends `cf-ipcountry` / `cf-ipcity` (or headers you rename to match) works.

## What's out of scope (for now)

- A GeoIP database for IP-to-city lookup without a proxy header. That would add a heavy dependency and a data file to keep updated; the header path already covers city for anyone running behind an edge that supports it.
- Bot and crawler filtering. Analytics currently count every request that hits the redirect, including bots. Filtering them out is a separate, later piece of work.
- Full per-URL referrer detail. Aggregates group by host; the raw referrer is still visible per-event in the recent-events ring if you need the exact URL.
