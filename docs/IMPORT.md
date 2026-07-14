**English** · [Português](IMPORT.PT_BR.md)

# Importing links (migration from another shortener)

`POST /admin/import` bulk-creates links from a CSV or JSON file. It exists
for one purpose: moving an existing link inventory into quark without
retyping every URL. Point it at an export from Bitly, Kutt, YOURLS, or any
generic spreadsheet, and it creates a link per row, reporting exactly which
rows succeeded and which failed.

The endpoint is admin-only (protected by `QUARK_ADMIN_TOKEN`), independent of
whether public `POST /` creation is enabled. Every row runs through the same
validation, blocklist, and anti-loop checks as a normal `POST /` create, so
an import can't create links that a manual create would have rejected.

## Formats

Send either JSON or CSV. quark picks the parser from the `Content-Type`
header first (`application/json`, or `text/csv`/`application/csv`); if the
header is missing or unrecognized, it sniffs the body: a leading `[` or `{`
is treated as JSON, anything else as CSV.

### JSON

An array of objects, one per link. `url` is required; `alias` and `ttl` are
optional.

```json
[
  { "url": "https://example.com/some/long/landing/page", "alias": "promo", "ttl": 604800 },
  { "url": "https://example.com/another/page" }
]
```

- `ttl` is in seconds, counted from import time (not preserved from the
  source system, which has no concept of "seconds until expiry" in an
  export).
- A row without `alias` gets a computed code from quark, same as a normal
  create.

### CSV

A header row, then one link per line. quark auto-detects the URL, alias,
and TTL columns by name (case-insensitive), so exports from different tools
work without pre-editing the file.

```csv
url,alias,ttl
https://example.com/some/long/landing/page,promo,604800
https://example.com/another/page,,
```

## Column and field mapping

quark recognizes several names per field, covering the vocabulary used by
Bitly, Kutt, and YOURLS exports:

| Field | JSON keys accepted | CSV headers accepted (case-insensitive) |
|---|---|---|
| URL (required) | `url`, `long_url`, `longUrl` | `url`, `long_url`, `longurl`, `original_url`, `long` |
| Alias / custom code (optional) | `alias`, `keyword`, `short` | `alias`, `keyword`, `short`, `short_code`, `custom` |
| TTL in seconds (optional) | `ttl` | `ttl`, `expiry` |

If a CSV has no column matching the URL list, the whole request is rejected
with `400 Bad Request` before any row is processed (there is nothing to
import). Missing alias or TTL columns are fine; those fields are simply
left empty for every row.

## Migrating from Bitly

Bitly's CSV export uses `long_url` for the destination and includes columns
quark ignores (click counts, creation date, etc).

1. In the Bitly dashboard, go to your links list and export to CSV.
2. Upload the CSV as-is (or paste its contents) into quark's Import page,
   or send it directly:

```bash
curl -X POST https://your-quark-host/admin/import \
  -H "content-type: text/csv" \
  -H "x-admin-token: $QUARK_ADMIN_TOKEN" \
  --data-binary @bitly_export.csv
```

Bitly's own short codes are not preserved (see "What is not preserved"
below); each link gets a fresh quark code unless you add an `alias` column
yourself before importing.

## Migrating from Kutt

Kutt can export either JSON or CSV, both of which quark reads directly.

- **JSON:** Kutt's link objects already use `target`-style URLs and a
  `code` field for the custom short code. Rename (or map) `code` to
  `alias`/`keyword`/`short` before uploading if your export doesn't already
  use one of those names, so quark treats it as the custom alias to keep.
- **CSV:** same idea; make sure the URL column is named `url` (or one of the
  accepted aliases above) and the short-code column is named `alias`,
  `keyword`, `short`, `short_code`, or `custom`.

```bash
curl -X POST https://your-quark-host/admin/import \
  -H "content-type: application/json" \
  -H "x-admin-token: $QUARK_ADMIN_TOKEN" \
  --data-binary @kutt_export.json
```

## Migrating from YOURLS

YOURLS' native CSV export already matches quark's expected shape closely:
its `keyword` column is the short code and its `url` column is the
destination, both recognized out of the box.

1. In YOURLS admin, use the "Export" tool (or the API) to get a CSV with a
   `keyword,url,...` header.
2. Import it without editing:

```bash
curl -X POST https://your-quark-host/admin/import \
  -H "content-type: text/csv" \
  -H "x-admin-token: $QUARK_ADMIN_TOKEN" \
  --data-binary @yourls_export.csv
```

Each `keyword` becomes the alias quark tries to reuse as the short code
(subject to the alias rules below).

## What is not preserved

Import only migrates the link itself (destination URL, optional alias,
optional TTL). It does not migrate click history or analytics from the
source system; that data stays behind in the old tool.

The original short code from the source system is kept **only** if it
arrives as an `alias`/`keyword` value and passes quark's alias rules (it
must not itself look like a valid quark computed code, and it must not
already be in use). If a row has no alias, or the alias is rejected, quark
assigns its own computed code instead of trying to preserve the source
system's code.

## The 10,000-row cap

A single `POST /admin/import` request accepts at most 10,000 rows. This is
a hard, synchronous request: there is no background job queue behind it, so
the cap bounds memory and runtime. A request with more rows than the cap is
rejected outright with `400 Bad Request`, before any row is imported. Split
a larger export into multiple files.

## Partial success and the failure report

Import never aborts on the first bad row. Every row is attempted
independently, using exactly the same validation as `POST /`. The response
is always `200 OK` (once the request itself is accepted) with a summary:

```json
{
  "imported": 2,
  "failed": [
    { "index": 3, "url": "not-a-url", "reason": "invalid url" },
    { "index": 7, "url": "https://example.com", "reason": "alias in use" }
  ]
}
```

- `imported` is the count of rows that created a link.
- `failed` lists every row that didn't, by its zero-based index in the
  request, its URL, and a short reason: `invalid url`, `url without host`,
  `blocked destination`, `alias collides with the numeric code space`,
  `alias in use`, `invalid ttl`, `id space exhausted`, or `backend error`.

Re-running the same file after fixing the failed rows is safe: rows that
already imported keep their links; only the previously failing rows need
attention (typically by editing the alias or URL and re-submitting just
those).

## Using the web panel

The panel's "Import" tab accepts a `.csv` or `.json` file upload, or a
block of text pasted directly (paste is useful for a quick JSON snippet or
a short CSV without saving a file first). After submitting, it shows
"Imported N, M failed" with a table of the failed rows (index, URL,
reason), so you can see exactly what to fix.

## curl reference

```bash
# JSON body
curl -X POST https://your-quark-host/admin/import \
  -H "content-type: application/json" \
  -H "x-admin-token: $QUARK_ADMIN_TOKEN" \
  -d '[{"url": "https://example.com/a", "alias": "promo", "ttl": 3600}, {"url": "https://example.com/b"}]'

# CSV body
curl -X POST https://your-quark-host/admin/import \
  -H "content-type: text/csv" \
  -H "x-admin-token: $QUARK_ADMIN_TOKEN" \
  --data-binary $'url,alias,ttl\nhttps://example.com/a,promo,3600\nhttps://example.com/b,,\n'
```

`x-admin-token` is required regardless of whether public `POST /` creation
is enabled: unset `QUARK_ADMIN_TOKEN` on the server means the endpoint
answers `404` (same as the other `/admin/*` endpoints); a wrong token
answers `401`.
