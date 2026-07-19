**English** · [Português](WEBHOOKS.PT_BR.md)

# Webhooks

A single-operator quark instance can push signed HTTP events to an external
endpoint (Zapier, Make, n8n, Slack, or any custom receiver). How delivery
behaves depends on the backend:

- On the **Postgres** backend, the lifecycle events (`link.created`,
  `link.updated`, `link.deleted`) are delivered **durably**: they land in a
  Postgres outbox and a leased relay worker delivers them at-least-once with
  persisted retry and a dead-letter queue. See [Durable delivery
  (Postgres)](#durable-delivery-postgres).
- `link.clicked` (and `link.expired`, which also fires on the redirect hot
  path) stay **best-effort** on every backend: an in-memory bounded queue
  feeds a worker that signs and POSTs with retry, and an event dropped on a
  full queue or a restart is simply missed. Adding a synchronous database
  write to the redirect path would defeat its whole purpose, so those two
  events are best-effort by design.
- On the **LMDB** single-node backend there is no outbox; every event,
  lifecycle included, rides the in-memory best-effort channel.

Subscription *configuration* (URL, event set, secret, active flag) is always
durable, persisted via the store (LMDB or Postgres), independent of how the
events themselves are delivered.

## Events

| Event | Fires when |
|---|---|
| `link.created` | A link is created (`POST /`). |
| `link.updated` | A link is edited (`PATCH /admin/links/:code`). |
| `link.deleted` | A link is removed (`DELETE /admin/links/:code`). |
| `link.expired` | A redirect resolves a link past its TTL (the `410 Gone` path). There is no background sweeper: expiry is observed on access, same as the rest of quark's TTL handling. |
| `link.clicked` | A redirect succeeds. Emitted from the async path, never the hot 302 path, and only when at least one active subscription wants it (a cached atomic flag keeps the cost at zero otherwise). |
| `link.threshold_reached` | A link is clicked at least `threshold` times within a fixed window of `window_secs` seconds, per the link's alert rule. Fires once per window (see [Click-threshold alerts](#click-threshold-alerts)). |

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
- `type`: one of the event names above.
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

`link.threshold_reached` carries the window that tripped the alert, on top of
`code`:

```json
{
  "id": "evt_...",
  "type": "link.threshold_reached",
  "timestamp": 1699999999,
  "data": {
    "code": "aB3xZ9k",
    "count": 100,
    "threshold": 100,
    "window_secs": 300,
    "ts": 1699999999
  }
}
```

- `count`: the click count that crossed the threshold in this window.
- `threshold` / `window_secs`: the configured rule.
- `ts`: unix seconds of the click that tripped the alert.

## Headers

Every request carries three headers, following the
[Standard Webhooks](https://www.standardwebhooks.com/) symmetric scheme:

| Header | Meaning |
|---|---|
| `webhook-id` | Stable per delivery, reused across every retry attempt for that delivery. Use it as the idempotency key: if you've already processed this id, skip it. |
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

## Durable delivery (Postgres)

On the Postgres backend the lifecycle events (`link.created`, `link.updated`,
`link.deleted`) do not ride the in-memory queue. Each one is written to a
durable outbox and handed to a relay worker, so a receiver that is down, or a
quark restart, no longer costs you the event.

**The outbox.** When a lifecycle event fires, quark writes one row per matching
active subscription into the `webhook_deliveries` table (event body, target
subscription, attempt count, next-attempt time, a dead flag). The write commits
before the admin request returns. If a subscription is down for an hour, its
rows sit in the outbox and are retried the whole time; nothing is lost to a
full queue.

**The leased relay.** A background worker polls the outbox on a short interval
and claims a batch of due rows with `SELECT ... FOR UPDATE SKIP LOCKED`. Two
things fall out of that:

- Run quark on several nodes and each relay claims a disjoint set of rows, so
  the same delivery is never sent twice by two nodes at once.
- A slow or broken endpoint only holds up its own rows. Other subscriptions
  are claimed and delivered in parallel, with no head-of-line blocking.

For each claimed row the relay looks up the subscription, applies the same SSRF
guard as every other delivery, signs the body (Generic) or formats it for the
channel (Slack/Discord/Telegram), and POSTs it. On a 2xx the row is marked
delivered. On a failure the attempt count is bumped and the next-attempt time
is pushed out with exponential backoff plus jitter, all persisted, so the
schedule survives a restart. After the attempt budget is exhausted the row is
flagged `dead`: it moves to the dead-letter queue and stops being claimed.

**At-least-once, and idempotency.** This is at-least-once delivery: a crash
between a successful POST and the row being marked delivered will redeliver on
the next poll. Deduplicate on the `webhook-id` header. For an outbox delivery
that header is the row's stable delivery key, `"<event_id>.<subscription_id>"`,
identical across every attempt and every node. Treat a repeated `webhook-id` as
a no-op (this is the same rule as [Replay and
idempotency](#replay-and-idempotency); the durable path just makes the id
stable and persisted rather than random per attempt).

**Enqueue is atomic with the mutation.** The outbox rows are inserted in the
same transaction as the link mutation that produced the event: the create,
patch, and delete paths build the matching delivery rows (a read of the active
subscriptions, outside the transaction) and hand them to the storage layer,
which writes the link change and the `ON CONFLICT (delivery_key) DO NOTHING`
inserts together. Either both commit or neither does, so a crash can no longer
lose an event between the link write and the outbox insert.

## Configuring a subscription

### Panel

Open the **Webhooks** tab in the admin panel. Add a destination URL, pick
which events it should receive, and save. The signing secret is
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

## Click-threshold alerts

A link can carry an alert rule: fire `link.threshold_reached` when it is
clicked at least `threshold` times within a fixed window of `window_secs`
seconds. This is handy for spotting a link that suddenly goes viral, or for
catching click fraud early.

Counting uses a fixed-window counter, the same approach as the rate limiter:
the window is `floor(click_ts / window_secs)`, and the event fires once per
window, on the click that first brings the window's count to `threshold`.
Further clicks in that same window do not re-fire; the next window starts a
fresh count and can fire again. When quark runs against Valkey the counter is
shared across every replica (exact cluster-wide); without Valkey each replica
counts on its own (exact on a single node). Counting and delivery run in the
analytics worker, off the redirect hot path, and are fail-open: a Valkey error
is logged and never blocks a redirect.

To receive the event, a webhook (or channel) subscription must include
`link.threshold_reached` in its `events`, exactly like any other event.

### Configuring the rule (API)

The rule is set per link, keyed by the link's short code, under
`QUARK_ADMIN_TOKEN` (header `x-admin-token`). `threshold` must be `>= 1` and
`window_secs` must be `>= 60`.

```bash
# set (or replace) the alert rule for a link:
# 100 clicks within 5 minutes
curl -X PUT localhost:8080/admin/links/aB3xZ9k/alert \
  -H 'x-admin-token: <token>' -H 'content-type: application/json' \
  -d '{"threshold": 100, "window_secs": 300}'
# => {"threshold": 100, "window_secs": 300}

# remove the alert rule
curl -X DELETE localhost:8080/admin/links/aB3xZ9k/alert -H 'x-admin-token: <token>'
# => 204 No Content
```

`:code` accepts either the canonical short code or a custom alias, the same as
the other `/admin/links/:code` endpoints.

### n8n template

Build a flow that reacts to `link.threshold_reached`:

1. **Webhook** node (trigger): method `POST`, copy its Test/Production URL, and
   register it as a subscription that includes the event:

   ```bash
   curl -X POST localhost:8080/admin/webhooks \
     -H 'x-admin-token: <token>' -H 'content-type: application/json' \
     -d '{"url": "https://<your-n8n-host>/webhook/quark", "events": ["link.threshold_reached"]}'
   ```

2. **IF** node (optional filter): continue only for a specific link, e.g.
   expression `{{ $json.body.data.code }}` equals `aB3xZ9k`.
3. **Action** node: Slack "Send Message", Email, or an HTTP Request. Reference
   the payload fields directly, for example:

   ```
   Link {{ $json.body.data.code }} hit {{ $json.body.data.count }} clicks
   in {{ $json.body.data.window_secs }}s.
   ```

To verify the signature inside n8n, add a **Code** node before the action and
port the [Node.js snippet](#nodejs) above, reading `webhook-id`,
`webhook-timestamp`, and `webhook-signature` from `$json.headers`.

### Zapier template

1. **Trigger**: "Webhooks by Zapier" -> "Catch Hook". Zapier gives you a
   custom URL; register it as a subscription with the event:

   ```bash
   curl -X POST localhost:8080/admin/webhooks \
     -H 'x-admin-token: <token>' -H 'content-type: application/json' \
     -d '{"url": "https://hooks.zapier.com/hooks/catch/000000/xxxxxx", "events": ["link.threshold_reached"]}'
   ```

2. **Filter** (optional): "Only continue if..." -> `Data Code` -> `(Text)
   Exactly matches` -> `aB3xZ9k`.
3. **Action**: any Zapier app, e.g. Slack "Send Channel Message" or Gmail
   "Send Email". Map the caught fields `data__code`, `data__count`, and
   `data__window_secs` into the message body.

Zapier's Catch Hook does not verify the HMAC signature. If you need
authenticity, use "Catch Raw Hook" and add a Code step that reimplements the
[verification](#verifying-the-signature), or keep the webhook URL secret and
rely on it being unguessable.

## Notification channels

A subscription has a `kind`: `generic` (default, described above) or one of
three chat channels, `slack`, `discord`, `telegram`. Pick a channel in the
create dialog's "Type" selector (or pass `kind` to the API) when all you want
is a plain message in a chat, not a signed integration.

The core difference: **channel kinds are not signed**. There is no HMAC, no
`webhook-*` headers, and no `secret`. Authentication is the URL itself: each
channel's incoming URL is a bearer credential, so anyone who has it can post
to your channel. Keep it as secret as you would a password. The SSRF guard
and the no-redirect policy still apply to channel URLs, same as Generic.

### Getting each channel's URL

**Slack.** Add the "Incoming Webhooks" app to your workspace (or open an
existing app's configuration), enable incoming webhooks, and create one for
the channel you want. Slack gives you a URL shaped like
`https://hooks.slack.com/services/T000/B000/XXXXXXXX`. Paste that as the
subscription URL.

**Discord.** In the target channel, open Server Settings > Integrations >
Webhooks, create a new webhook, and copy its URL
(`https://discord.com/api/webhooks/<id>/<token>`). Paste that as the
subscription URL.

**Telegram.** Message [@BotFather](https://t.me/BotFather) to create a bot
and get its token. Find the numeric `chat_id` of the chat you want messages
in (a private chat, group, or channel your bot is a member of). Build the
URL yourself:

```
https://api.telegram.org/bot<TOKEN>/sendMessage?chat_id=<ID>
```

Paste that whole URL as the subscription URL. quark POSTs `text` in the JSON
body; Telegram reads `chat_id` from the query string and `text` from the
body.

### Message format

quark derives a short plain-text message from the same event data that
Generic subscriptions receive, then wraps it in the shape each channel
expects:

| Event | Message |
|---|---|
| `link.created` | `New short link: {code} -> {url}` |
| `link.updated` | `Short link updated: {code} -> {url}` |
| `link.deleted` | `Short link deleted: {code}` |
| `link.expired` | `Short link expired: {code}` |
| `link.clicked` | `Click on {code} -> {url}`, with ` ({country})` appended when the click carried a country |
| `link.threshold_reached` | `Click threshold reached for {code}: {count} clicks in {window_secs}s` |

Slack and Telegram both receive:

```json
{"text": "New short link: aB3xZ9k -> https://example.com/dest"}
```

Discord receives:

```json
{"content": "New short link: aB3xZ9k -> https://example.com/dest"}
```

This is plain text, no formatting markup. Slack's Block Kit and Discord's
rich embeds are richer message formats both channels support; quark doesn't
build them today. That's a future formatting upgrade, not something you can
opt into now.

### API

Channel subscriptions use the same endpoints as Generic, with `kind` in the
create request:

```bash
curl -X POST localhost:8080/admin/webhooks \
  -H 'x-admin-token: <token>' -H 'content-type: application/json' \
  -d '{"url": "https://hooks.slack.com/services/T000/B000/XXXXXXXX", "events": ["link.created"], "kind": "slack"}'
# => {"id": 2}   (no "secret" field: channel kinds aren't signed)
```

`kind` is one of `"generic"`, `"slack"`, `"discord"`, `"telegram"`; it
defaults to `"generic"` when omitted.
