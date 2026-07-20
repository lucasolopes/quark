**English** · [Português](API.PT_BR.md)

# HTTP API reference

Every route quark serves, from `src/api/router.rs` (`router_with_cors`). Two routes are
public by default (`POST /` and `GET /:code`); the well-known files are always
public; everything under `/admin/*` and `GET /:code/stats` is gated.

Timestamps (`created`, `expiry`, `ts`, `timestamp`) are unix seconds. A `code`
is either a computed 7-character base62 string or a custom alias.

## Authentication

Admin requests carry the token in the `x-admin-token` header. Two kinds of
token are accepted:

- The env `QUARK_ADMIN_TOKEN`, compared in constant time. It always has the
  `full` scope.
- A named API token (`qtok_...`), looked up by its SHA-256 hash. It is allowed
  only if its scopes cover the scope the endpoint requires, and it may carry a
  per-token rate limit. See [API-TOKENS](API-TOKENS.md).

The scope each admin endpoint requires is listed per route below. Shared status
codes when a scope check fails:

| Status | When |
|---|---|
| `401 Unauthorized` | Token missing or unknown, and `QUARK_ADMIN_TOKEN` is configured. |
| `403 Forbidden` | Valid API token whose scopes do not cover the required one. |
| `404 Not Found` | Token missing or unknown, and no `QUARK_ADMIN_TOKEN` is configured (the admin surface is fully disabled). |
| `429 Too Many Requests` | Valid API token over its own `rate_limit_per_min`. |
| `503 Service Unavailable` | The store failed while checking the token. |

## Public routes

### `GET /health`

Liveness check. No auth. Returns `200` with the body `ok`.

### `POST /`

Create a short link. Public when `QUARK_ADMIN_TOKEN` is unset; otherwise it
requires the env token or an API token that covers `links_write`.

Request body (`application/json`):

| Field | Type | Notes |
|---|---|---|
| `url` | string, required | Must start with `http://` or `https://`. |
| `alias` | string, optional | Custom code. Must not itself be a valid in-range 7-char base62 code. |
| `ttl` | number, optional | Seconds until expiry, from now. |
| `tags` | string array, optional | Normalized: trimmed, lowercased, deduped, capped at 20. |
| `max_visits` | number, optional | Expire after this many visits. `0` or absent means unlimited. |
| `rules` | array, optional | Geo/device redirect rules, up to 20. See [REDIRECT-RULES](REDIRECT-RULES.md). |
| `variants` | array, optional | Weighted A/B variants, up to 10. See [AB-TESTING](AB-TESTING.md). |
| `app_ios` | string, optional | iOS deep-link destination. See [DEEP-LINKING](DEEP-LINKING.md). |
| `app_android` | string, optional | Android deep-link destination. |
| `folder` | string, optional | One folder this link belongs to. Trimmed, capped at 48 chars, case preserved; empty means none. |
| `fallback_url` | string, optional | Where to send visitors once the link has expired (by TTL or `max_visits`) instead of `410`. `http`/`https`, non-internal; empty means none. |
| `password` | string, optional | Protect the link with a password. Stored as an argon2id hash; the plaintext is never persisted or returned. Empty/absent means no password. |

Success: `200` with `{"code": "...", "url": "..."}`.

Failures (each is a plain-text body):

| Status | Reason |
|---|---|
| `400 Bad Request` | invalid url, url without host, invalid ttl, alias collides with the numeric code space, too many rules, too many variants, invalid device value, variant weight must be >= 1 |
| `403 Forbidden` | blocked destination (internal/self target), for the url, a rule `to`, or a variant url |
| `409 Conflict` | alias in use |
| `429 Too Many Requests` | over `QUARK_RATELIMIT_PER_MIN` for the client IP |
| `507 Insufficient Storage` | id space exhausted |
| `503 Service Unavailable` | backend error |

```bash
curl -X POST localhost:8080/ -H 'content-type: application/json' \
  -d '{"url": "https://example.com/some/long/path", "ttl": 3600}'
# => {"code":"01aB2Cd","url":"https://example.com/some/long/path"}
```

### `GET /:code`

Resolve and redirect. No auth. quark decodes a numeric base62 code by
arithmetic first, then falls back to an alias lookup.

Responses:

| Status | When | Headers |
|---|---|---|
| `302 Found` | Link resolved and live. | `Location`, TTL-aware `Cache-Control`. |
| `302 Found` | Expired (TTL or `max_visits`) and a `fallback_url` is set. | `Location: <fallback_url>`, `Cache-Control: no-store`. |
| `410 Gone` | Expired (TTL or `max_visits`) with no `fallback_url`. | `Cache-Control: no-store`. |
| `200 OK` | Link is password-protected and the request has no valid unlock cookie. | `text/html` interstitial, `Cache-Control: no-store`. |
| `404 Not Found` | No such code or alias. | `Cache-Control: no-store`. |
| `503 Service Unavailable` | Backend error. | |

Destination resolution composes three targeting mechanisms in priority order:
a device-aware app deep-link wins first, then a matching geo/device rule, then a
weighted A/B variant, and a link with none of these redirects to its `url`. See
[ARCHITECTURE](ARCHITECTURE.md#redirect-flow).

### `POST /:code`

Unlock a password-protected link. Public, rate-limited (per client IP, shared
with create). Body is `application/x-www-form-urlencoded` with a `password`
field.

| Status | When | Headers |
|---|---|---|
| `303 See Other` | Correct password. | `Location: /<code>`, `Set-Cookie: qk_pw_<code>=…` (signed, `HttpOnly`, `SameSite=Lax`, 12h), `Cache-Control: no-store`. |
| `200 OK` | Wrong password. | `text/html` interstitial with an error, no cookie. |
| `429 Too Many Requests` | Over the rate limit. | |

On success quark redirects back to `GET /:code`; the follow-up request carries
the unlock cookie, so destination resolution, the visit bump, and click
recording all happen once on the canonical redirect path. The cookie lets repeat
visits within 12 hours skip the interstitial. The password itself is verified
against an argon2id hash; the plaintext is never stored.

### Well-known files (deep linking)

Public, served as `application/json` with no redirect. `200` with the stored
body, or `404` when unset. See [DEEP-LINKING](DEEP-LINKING.md).

| Route | File |
|---|---|
| `GET /.well-known/apple-app-site-association` | iOS AASA |
| `GET /apple-app-site-association` | iOS AASA (legacy root path) |
| `GET /.well-known/assetlinks.json` | Android Digital Asset Links |

## Analytics

### `GET /:code/stats`

Per-link click analytics. Scope: `analytics`. `404` if the code does not
resolve to a stored link.

Success: `200` with `{"aggregates": {...}, "recent": [...]}`. `aggregates`
holds `total`, `bots`, `first_ts`, `last_ts`, and the `per_day`, `per_country`,
`per_device`, `per_os`, `per_browser`, `per_referer`, `per_city`, `per_variant`
maps. `recent` is the newest click events (up to 1000). A link with no clicks
returns empty aggregates and an empty list. See [ANALYTICS](ANALYTICS.md).

## Link management

### `GET /admin/links`

List links, keyset-paginated. Scope: `links_read`.

Query parameters: `after` (id cursor), `limit` (default 50, clamped to 500),
`q` (search over url and alias, Postgres only), `tag` (filter by one tag),
`folder` (filter by folder name, case-insensitive).

Success: `200` with `{"links": [...], "next_after": <id or null>}`. Each link
row carries `id`, `code`, optional `alias`, `url`, `expiry`, `created`, `tags`,
optional `max_visits`, `visits`, `rules`, `variants`, an optional `folder`
(omitted when the link has none), an optional `fallback_url`,
`has_password` (a bool; the password hash itself is never returned), and an
optional `health` object (`{healthy, status, checked_at}`, omitted when the link
was never probed; see [LINK-HEALTH](LINK-HEALTH.md)).

Query parameters include `after`, `limit`, `q`, `tag`, `folder`, and
`health=broken` (only links whose last health probe failed).

`501 Not Implemented` is returned when `q` is used on the LMDB backend (search
is Postgres-only; the panel falls back to client-side filtering).

### `GET /admin/tags`

The distinct tags across all links with their link counts, for the panel's
filter. Scope: `links_read`. Returns
`{"tags": [{"name": "...", "count": N}, ...]}`, sorted by name. A tag repeated
on the same link counts that link once.

### `GET /admin/folders`

The distinct folder names with their link counts, for the panel's folder
selector and filter. Scope: `links_read`. Returns
`{"folders": [{"name": "...", "count": N}, ...]}`, sorted by name. Links with no
folder are not counted.

### `PATCH /admin/links/:code`

Edit a link. Scope: `links_write`. The body is a partial JSON object; only the
keys present are changed. Sending `null` (or, for `fallback_url`/`password`, an
empty string) for `ttl`, `max_visits`, `app_ios`, `app_android`, `folder`,
`fallback_url`, or `password` clears that field. A non-empty `password` sets a
new hash.

Accepted keys: `url`, `ttl`, `tags`, `max_visits`, `rules`, `variants`,
`app_ios`, `app_android`, `folder`, `fallback_url`, `password`. Each is validated the same way as on create
(URL scheme, SSRF guard, rule and variant caps; the folder name is trimmed and
capped). `200` on success, `404` if the code does not resolve, `400`/`403` on a
rejected field.

### `DELETE /admin/links/:code`

Remove a link (and its alias, if the code was an alias). Scope: `links_write`.
`200` on success, `404` if it does not resolve.

### `POST /admin/import`

Bulk-create links from a CSV or JSON body. Scope: `links_write`. Always
admin-gated, even when public `POST /` is enabled. Never aborts on a bad row;
returns `200` with `{"imported": N, "failed": [{index, url, reason}, ...]}`.
A body over 10,000 rows or an unparseable body is `400`. See [IMPORT](IMPORT.md).

## Webhooks

Subscription management. Scope: `webhooks` on every route. See
[WEBHOOKS](WEBHOOKS.md) for events, payload, and signing.

| Route | Purpose |
|---|---|
| `GET /admin/webhooks` | List subscriptions (secret masked). `200 {"webhooks": [...]}`. |
| `POST /admin/webhooks` | Create. Body `{url, events, active?, kind?}`. `201 {"id", "secret"?}`. The signing secret is returned once, only for `generic`. |
| `PATCH /admin/webhooks/:id` | Update `url`, `events`, `active`, or `kind`. `200`, or `404`. |
| `DELETE /admin/webhooks/:id` | Remove. `204`, or `404`. |
| `POST /admin/webhooks/:id/test` | Deliver a synthetic `link.created` once, synchronously. Returns `{"delivered": bool, "status"?: N, "error"?: "..."}`. |

`url` must be `http`/`https` and pass the SSRF guard. A deployment caps at 50
subscriptions (`400` beyond it). `kind` is `generic` (default, signed), `slack`,
`discord`, or `telegram` (unsigned channel messages).

## Conversion pixels

Instance-level GA4 / Meta CAPI configs. Scope: `analytics`. See
[CONVERSION-FORWARDING](CONVERSION-FORWARDING.md).

| Route | Purpose |
|---|---|
| `GET /admin/pixels` | List configs, secrets masked. `200 {"pixels": [...]}`. |
| `POST /admin/pixels` | Create. Body `{provider, credentials, active?}` where `provider` is `ga4` or `meta_capi`. `201` with the masked row, or `400` on missing required credentials or the 20-config cap. |
| `DELETE /admin/pixels/:id` | Remove. `204`, or `404`. |

## API tokens

Token management. Scope: `full` on every route (only a superuser token manages
tokens). See [API-TOKENS](API-TOKENS.md).

| Route | Purpose |
|---|---|
| `GET /admin/tokens` | List tokens (never the hash or plaintext). `200 {"tokens": [...]}`. |
| `POST /admin/tokens` | Create. Body `{name, scopes, rate_limit_per_min?}`. `201 {"id", "token"}` with the plaintext once. `400` on the 100-token cap. |
| `DELETE /admin/tokens/:id` | Revoke. `204`, or `404`. |

## Well-known documents (admin)

Manage the deep-linking association files. Scope: `full`. `:name` must be
`apple-app-site-association` or `assetlinks.json`, else `404`.

| Route | Purpose |
|---|---|
| `GET /admin/wellknown/:name` | Read the stored body. `200` with the body, or `200` with an empty body when unset (the panel treats empty as "not configured"). |
| `PUT /admin/wellknown/:name` | Store a body. Must be valid JSON within 64 KiB, else `400`. `200` on success. |
| `DELETE /admin/wellknown/:name` | Remove it; the public path goes back to `404`. `204`. |

## CORS

When `QUARK_CORS_ORIGINS` is set (comma-separated), quark adds a CORS layer
allowing those origins with `GET, POST, PUT, PATCH, DELETE` and any headers.
Unset means same-origin only. This is for the separately hosted web panel.
