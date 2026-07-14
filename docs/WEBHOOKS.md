**English** · [Português](WEBHOOKS.PT_BR.md)

# Webhooks

A single-operator quark instance can push signed HTTP events to an external
endpoint (Zapier, Make, n8n, Slack, or any custom receiver). Delivery is
best-effort: events go through an in-memory bounded queue, a worker signs and
POSTs them with retry (exponential backoff plus jitter), and a subscription
that stays down past the retry budget just misses that event. Subscription
*configuration* (URL, event set, secret, active flag) is durable, persisted
via the store (LMDB or Postgres); delivery itself is not.

If you need durable, restart-surviving delivery with a persisted attempt log,
that's a future Postgres-gated enhancement, not what this covers today.

## Events

| Event | Fires when |
|---|---|
| `link.created` | A link is created (`POST /`). |
| `link.updated` | A link is edited (`PATCH /admin/links/:code`). |
| `link.deleted` | A link is removed (`DELETE /admin/links/:code`). |
| `link.expired` | A redirect resolves a link past its TTL (the `410 Gone` path). There is no background sweeper: expiry is observed on access, same as the rest of quark's TTL handling. |
| `link.clicked` | A redirect succeeds. Emitted from the async path, never the hot 302 path, and only when at least one active subscription wants it (a cached atomic flag keeps the cost at zero otherwise). |

## Payload

Every delivery has the same envelope:

```json
{
  "id": "evt_...",
  "type": "link.created",
  "timestamp": 1699999999,
  "data": {
    "code": "aB3xZ9k",
    "url": "https://example.com/dest",
    "alias": "promo",
    "expiry": 1700003599,
    "created": 1699990000
  }
}
```

- `id`: a random event id, distinct per emission.
- `type`: one of the five event names above.
- `timestamp`: unix seconds, when the event was generated.
- `data.alias` and `data.expiry` are omitted (not sent as `null`) when the
  link has no alias or no TTL.

`link.clicked` carries the same click context already captured for analytics,
on top of `code`, `url`, and `created`:

```json
{
  "id": "evt_...",
  "type": "link.clicked",
  "timestamp": 1699999999,
  "data": {
    "code": "aB3xZ9k",
    "url": "https://example.com/dest",
    "created": 1699990000,
    "country": "BR",
    "device": "Mobile",
    "referrer": "https://twitter.com/",
    "ts": 1699999999
  }
}
```

`country` and `referrer` are omitted when the request didn't carry them.
`device` is always present (falls back to `"Other"` when the user agent
can't be classified).

## Headers

Every request carries three headers, following the
[Standard Webhooks](https://www.standardwebhooks.com/) symmetric scheme:

| Header | Meaning |
|---|---|
| `webhook-id` | Unique per delivery attempt. Use it as the idempotency key: if you've already processed this id, skip it. |
| `webhook-timestamp` | Unix seconds when the request was signed. Reject requests where this is more than 5 minutes old (or noticeably in the future): that's the replay window. |
| `webhook-signature` | `v1,<base64>`. A space-delimited list of `v1,...` entries; checking any one that matches is enough. Multiple entries only appear during secret rotation. |

## Verifying the signature

The signed string is `{webhook-id}.{webhook-timestamp}.{body}` (literal dots,
the exact request body bytes, not a re-serialized copy of it). The secret is
displayed and stored as `whsec_<base64>`; the key you feed to HMAC is the
raw bytes after base64-decoding everything past the `whsec_` prefix.

`signature = "v1," + base64(HMAC-SHA256(key, signed_string))`

### Node.js

```js
const crypto = require('crypto');

function verifyWebhook(secret, webhookId, timestamp, body, signatureHeader) {
  const now = Math.floor(Date.now() / 1000);
  if (Math.abs(now - Number(timestamp)) > 300) {
    throw new Error('timestamp outside the 5-minute tolerance');
  }

  const signedString = `${webhookId}.${timestamp}.${body}`;
  const key = Buffer.from(secret.replace(/^whsec_/, ''), 'base64');
  const expected = crypto.createHmac('sha256', key).update(signedString).digest();

  const candidates = signatureHeader.split(' ').map((entry) => entry.split(',')[1]);
  return candidates.some((candidate) => {
    const candidateBuf = Buffer.from(candidate, 'base64');
    return candidateBuf.length === expected.length && crypto.timingSafeEqual(candidateBuf, expected);
  });
}

// Usage in an Express handler:
// const ok = verifyWebhook(secret, req.header('webhook-id'), req.header('webhook-timestamp'),
//   rawBody, req.header('webhook-signature'));
```

`rawBody` must be the untouched request body string (or buffer), captured
before any JSON parsing middleware rewrites it.

### Python

```python
import base64
import hashlib
import hmac
import time


def verify_webhook(secret: str, webhook_id: str, timestamp: str, body: str, signature_header: str) -> bool:
    now = int(time.time())
    if abs(now - int(timestamp)) > 300:
        raise ValueError("timestamp outside the 5-minute tolerance")

    signed_string = f"{webhook_id}.{timestamp}.{body}"
    key = base64.b64decode(secret.removeprefix("whsec_"))
    expected = hmac.new(key, signed_string.encode(), hashlib.sha256).digest()

    for entry in signature_header.split(" "):
        _, candidate = entry.split(",", 1)
        if hmac.compare_digest(base64.b64decode(candidate), expected):
            return True
    return False
```

`body` must be the exact request body string your framework hands you before
any JSON deserialization, since a re-serialized copy can differ byte-for-byte
(key order, whitespace) and would fail the comparison even for a genuine
event.

### Test vector

This is the standard test vector used to check the two snippets above
against quark's own signer:

```
secret:    whsec_MfKQ9r8GKYqrTwjUPD8ILPZIo2LaLaSw
webhook-id:        msg_p5jXN8AQM9LWM0D4loKWxJek
webhook-timestamp: 1614265330
body:      {"test": 2432232314}
signature: v1,g0hM9SsE+OTPJTGt/tmIKtSyZlE3uFJELVlNIOLJ1OE=
```

Note the literal space after the colon in the body; a compact
`{"test":2432232314}` produces a different signature.

## Replay and idempotency

- Reject any delivery whose `webhook-timestamp` is more than 5 minutes old.
  A captured, replayed request outside that window should be dropped.
- Use `webhook-id` as your idempotency key. quark's own retry logic can
  redeliver the same event (network timeout, 5xx response, and so on), so
  your receiver should treat a repeated id as a no-op, not a duplicate
  side effect.

## Configuring a subscription

### Panel

Open the **Webhooks** tab in the admin panel. Add a destination URL, pick
which of the five events it should receive, and save. The signing secret is
shown once, at creation time; copy it before leaving the screen, since quark
never displays it again (only a masked `whsec_••••` afterward).

### API

All webhook endpoints live under `QUARK_ADMIN_TOKEN` (header `x-admin-token`).

```bash
# create a subscription
curl -X POST localhost:8080/admin/webhooks \
  -H 'x-admin-token: <token>' -H 'content-type: application/json' \
  -d '{"url": "https://hooks.example.com/quark", "events": ["link.created", "link.clicked"]}'
# => {"id": 1, "secret": "whsec_..."}   (the secret is returned only this once)

# list subscriptions (secret masked)
curl localhost:8080/admin/webhooks -H 'x-admin-token: <token>'

# update events or the active flag
curl -X PATCH localhost:8080/admin/webhooks/1 \
  -H 'x-admin-token: <token>' -H 'content-type: application/json' \
  -d '{"active": false}'

# remove a subscription
curl -X DELETE localhost:8080/admin/webhooks/1 -H 'x-admin-token: <token>'

# send a synthetic test event to check your receiver
curl -X POST localhost:8080/admin/webhooks/1/test -H 'x-admin-token: <token>'
```

`url` must be `http` or `https` and must not resolve to an internal or
loopback host: the same SSRF guard (`is_internal_host`) that protects link
destinations applies here, checked both at subscription-create time and
again at delivery time. A deployment caps out at 50 subscriptions.
