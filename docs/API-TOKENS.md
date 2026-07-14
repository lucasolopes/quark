**English** · [Português](API-TOKENS.PT_BR.md)

# API tokens

Beyond the single `QUARK_ADMIN_TOKEN`, quark supports named API tokens with
per-permission scopes and an optional per-token rate limit. This lets a
script, CI pipeline, or integration hold a token narrower than full admin
access, and lets you revoke that one integration without rotating the
operator's own token.

## Scopes

Each token is granted one or more scopes. A request is allowed only if the
token's scopes cover what the endpoint requires.

| Scope | Grants |
|---|---|
| `links_read` | List links (`GET /admin/links`), including search. |
| `links_write` | Create, edit, and delete links (`POST /`, `PATCH`/`DELETE /admin/links/:code`, import, tag writes). |
| `webhooks` | Manage webhook configuration. |
| `analytics` | Read click stats (`GET /:code/stats`). |
| `full` | Superuser: covers every scope above, including token management (`/admin/tokens`). Only `full` tokens can create, list, or revoke other tokens. |

The env `QUARK_ADMIN_TOKEN` always behaves as `full`, unchanged from before
API tokens existed.

## Using a token

Send the token in the `x-admin-token` header, exactly like the env admin
token:

```bash
# create a token (requires a full/superuser token, e.g. QUARK_ADMIN_TOKEN)
curl -X POST https://your-quark-host/admin/tokens \
  -H 'x-admin-token: <admin-token>' \
  -H 'content-type: application/json' \
  -d '{"name": "CI pipeline", "scopes": ["links_read"], "rate_limit_per_min": 60}'
# => 201 {"id": 3, "token": "qtok_...32+ chars..."}
# The plaintext token is returned ONLY in this response. Copy it now.

# use the token on a scoped endpoint
curl https://your-quark-host/admin/links \
  -H 'x-admin-token: qtok_...'
```

A token whose scopes don't cover the endpoint gets `403 Forbidden`. A revoked
or unknown token gets `401 Unauthorized` (or `404` if no env admin token is
configured at all, matching the existing env-token behavior).

## Quota (per-token rate limit)

A token can optionally carry `rate_limit_per_min`. Once set, requests
authenticated with that token are counted per minute; exceeding the limit
returns `429 Too Many Requests`. Leaving it unset means no per-token limit
(the token is still subject to any global rate limiting quark has configured
separately).

## The token is shown once

The plaintext token is generated on `POST /admin/tokens` and returned exactly
once, in that response. quark stores only its SHA-256 hash, never the
plaintext. `GET /admin/tokens` (the list) and any other endpoint never return
the hash or the plaintext, only `id`, `name`, `scopes`, `rate_limit_per_min`,
and `created`. If you lose the plaintext, there is no way to recover it:
revoke the token and create a new one.

## Managing tokens

The `/admin/tokens` endpoints themselves require a `full` scope token (or the
env `QUARK_ADMIN_TOKEN`):

- `GET /admin/tokens`: list tokens (no hash or plaintext).
- `POST /admin/tokens` `{name, scopes, rate_limit_per_min?}`: create a token,
  returns `201 {id, token}` with the plaintext once.
- `DELETE /admin/tokens/:id`: revoke a token, returns `204`.

The web panel's **API tokens** page (`/tokens`) wraps all three: create with
a name, scope checkboxes, and an optional rate limit; the plaintext token is
shown once with a copy button and a warning that it will not be shown again;
each token in the list can be revoked with a confirmation dialog.

Up to 100 tokens may exist at once.
