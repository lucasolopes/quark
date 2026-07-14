**English** · [Português](REDIRECT-RULES.PT_BR.md)

# Redirect rules (geo/device)

Roadmap #12. A single short link can resolve to different destinations
depending on who's clicking: their country, or the kind of device they're
on. This is a Shlink-style rule engine, built on top of the data quark
already reads for click analytics.

## The model

Each link has an ordered list of rules, plus its normal `url`:

- **Rules are evaluated in order.** The first rule whose condition matches
  the visitor wins.
- **`url` is the default.** If no rule matches (or the link has no rules
  at all), the visitor goes to `url`, exactly like before this feature
  existed.
- A rule never changes the link's code, alias, or analytics. It only
  changes which destination a given click resolves to.

A rule has three parts:

```json
{ "field": "country", "values": ["BR", "PT"], "to": "https://example.com/lp-pt" }
```

- `field`: what to match on. `"country"` or `"device"`.
- `values`: the list of values that make this rule match. A rule matches
  if the visitor's value is in this list.
- `to`: the destination to redirect to when this rule matches. Validated
  the same way as the main `url` (must be `http://` or `https://`, and
  cannot point at an internal/blocked host).

## Fields

### `country`

Matched against the two-letter ISO country code of the visitor, for
example `BR`, `US`, `PT`. The API uppercases whatever you send, so
`br` and `BR` are equivalent.

**This requires the edge in front of quark to send a `cf-ipcountry`
header** (Cloudflare does this automatically for any request that passes
through it). Without that header, quark has no way to know the visitor's
country, and country rules simply never match, falling through to the
default `url`. See `docs/EDGE.md` for the current edge setup.

### `device`

Matched against a coarse device category, derived from the visitor's
`User-Agent`: `Mobile`, `Desktop`, or `Other`. The API normalizes casing,
so `mobile` becomes `Mobile`. OS- and browser-level rules (e.g. "iOS
only") are out of scope for now; they need finer User-Agent parsing that
isn't part of quark yet.

## Limits

- Up to 20 rules per link.
- Rules are optional. Most links have none, and pay nothing extra on
  redirect: the check for "does this link have rules" is a single
  `is_empty()`, same cost either way.

## Example

A link with `url: "https://example.com"` and two rules:

```json
[
  { "field": "country", "values": ["BR"], "to": "https://example.com/br" },
  { "field": "device", "values": ["Mobile"], "to": "https://example.com/m" }
]
```

- A visitor from Brazil, on any device: goes to `https://example.com/br`
  (the country rule is first and matches).
- A visitor from the US, on a phone: no country match, falls to the
  device rule, goes to `https://example.com/m`.
- A visitor from the US, on a desktop: no rule matches, goes to the
  default `https://example.com`.

## Managing rules

In the web panel, both the create and edit link dialogs have a
collapsible "Redirect rules" section. Add a row per rule, choose the
field, type the values (comma-separated, e.g. `BR, PT`), and set the
destination. The link's own URL field stays the default destination and
is unaffected by the rules section.

## API

`POST /` (create) and `PATCH /admin/links/:code` both accept an optional
`rules` array in the request body, in the shape described above.
`GET /admin/links` returns each link's current `rules`.
