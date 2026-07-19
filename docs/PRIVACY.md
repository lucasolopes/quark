**English** · [Português](PRIVACY.PT_BR.md)

# Privacy

This document describes, precisely, what quark captures about a visitor who
clicks a short link, what it stores, what it sends off-box, and what quark
does when a visitor's browser sends a privacy opt-out signal. It is written
for the operator running quark, to use as the factual basis for your own
privacy notice. It does not give legal advice; the lawful basis for
processing and your notice to end users remain your responsibility.

## What happens on a click

The redirect (`GET /:code`) is a plain server response: a `302` with a
`Location` header and a `Cache-Control` header. It sets no cookie on the
visitor's browser, runs no client-side script, and makes no third-party
request from the visitor's device. A visitor who clicks a normal short link
has nothing written to or read from their browser.

quark's click analytics are built entirely from data already present on that
one request, server-side:

- `country` and `city`, from CDN geo headers.
- `referer`, from the `Referer` header.
- `user_agent`, from the `User-Agent` header.
- `ip`, from the configured real-IP header (or the socket).
- `fbc`, derived from a `fbclid` query parameter, when present.

This is cookieless analytics: there is no persistent identifier tying one
click to another click by the same visitor. Each click is captured and
counted on its own.

## What gets written to disk

`ip` and `fbc` are never persisted. They exist in memory only, for the
duration of that one click's processing, and are used solely to forward a
conversion event to a pixel provider when the operator has configured one
(see below). They are excluded from serialization at the code level, so
there is no accidental path that writes them to the stored analytics buffer.

What does get written: `user_agent`, `referer`, `country`, `city`, and the
click timestamp, plus a per-click event id. This raw per-click buffer is
capped at 1000 events per link and trimmed on a rolling basis. On the shared
Postgres backend it can also be purged on a time-based schedule (see
Retention below). Aggregated counts (by country, city, device, browser,
referer host, and day) are kept alongside the raw buffer and carry no
per-click identifying detail.

## Retention

The raw per-click detail (the near-PII buffer above) can be purged on a
time-based schedule. The default depends on how quark runs:

- **OSS / self-host** (single-tenant, the default): retention is
  **unlimited**. Nothing is purged by time; the only bound is the rolling
  1000-event-per-link cap.
- **Cloud** (multi-tenant, `QUARK_MULTI_TENANT`): retention defaults to
  **365 days**.

Either default is overridden by `QUARK_ANALYTICS_RETENTION_DAYS`: set it to a
number of days to enable (or shorten/lengthen) the purge, on any mode. When a
retention window is in effect, a background task runs about once an hour and
deletes click-detail rows older than the window; it fails open (a purge error
is logged and never affects redirects or serving). Setting the variable to a
non-numeric value, or leaving it unset in OSS mode, means unlimited (no purge
task runs).

Retention only prunes the raw per-click detail (`click_events`). The
aggregated counts (`click_counters` / `stats_meta`) are **not** pruned: they
carry no per-click identifying detail and are what the analytics view is
built from. On the embedded LMDB backend the detail buffer is already bounded
per link, so the purge simply prunes that ring by timestamp when a window is
configured.

When ClickHouse is used as the analytics sink (a later release, LUC-54),
retention there is enforced by a native table TTL rather than this task; the
`QUARK_ANALYTICS_RETENTION_DAYS` window maps onto that TTL when ClickHouse is
provisioned.

## Erasing a link's analytics

`DELETE /admin/links/:code/analytics` erases all analytics for a single link:
the raw per-click detail, the aggregated counts, and the stats metadata, all
scoped to the caller's tenant. It requires the `links:write` scope (the same
credential that can edit or delete the link). It does **not** delete the link
itself: the short code keeps redirecting; only its analytics are removed. A
successful erasure returns `204 No Content`. This is the API surface for a
per-link erasure request; there is no admin-panel UI for it in this release.

## Global Privacy Control (GPC)

quark honors the `Sec-GPC` request header automatically. There is no setting
to turn this off: it is on by default for every deployment.

When a visitor's browser sends `Sec-GPC: 1` on a click:

- The redirect still happens. The visitor reaches their destination exactly
  as they would otherwise.
- The click is not written to quark's analytics. No `ClickEvent` is recorded
  for that click.
- The click is not forwarded to any configured conversion pixel (GA4, Meta).

Both suppressions come from the same code path, so honoring GPC once covers
analytics capture and conversion forwarding together.

What GPC does **not** change:

- The redirect itself, and its `Cache-Control` behavior.
- A link's `max_visits` counter: a visit still counts against the limit, and
  a link still expires once its limit is reached, whether or not GPC was
  sent. This is link lifecycle enforcement, not visitor tracking.
- The `link.clicked` webhook, if the operator has subscribed to it. That
  webhook is a first-party notification to the operator's own endpoint about
  activity on their own link, not third-party tracking of the visitor, so it
  is unaffected by GPC.

GPC has real legal standing as an opt-out signal in a growing number of
jurisdictions. DNT (`Do Not Track`) has no such standing today and is not
read by quark.

## The one cookie a visitor can get

The only cookie quark ever sets on a visitor's browser is `qk_pw_<code>`,
and only after that visitor submits the correct password for a
password-protected link. It is `HttpOnly`, `SameSite=Lax`, `Secure` over
HTTPS, HMAC-signed, scoped to that one link, and expires after 12 hours. It
carries no identifier that would let quark or anyone else recognize the same
visitor across links; it exists only to remember "this browser already
entered the password for this link." This is a functional cookie, set in
direct response to the visitor's own action, in the same category as a
login or a shopping-cart cookie.

## Cookies outside visitor-consent scope

The admin panel sets its own first-party cookies for the operator's login
session (OIDC session, login state, and the Sheets OAuth state cookie during
the integration flow). These authenticate the operator using quark's admin
panel, not the visitor clicking a short link, and sit entirely outside the
visitor-consent conversation.

## Conversion forwarding: the one place data leaves quark

If the operator configures a GA4 or Meta pixel (the **Pixels** page), each
click is additionally forwarded, server-side and after the redirect has
already completed, to that provider:

- **GA4** receives only the link's short code, country, timestamp, and a
  synthetic per-instance client id. No IP, no User-Agent.
- **Meta CAPI** additionally receives the client IP, raw User-Agent, and
  `fbc` (all in plaintext, since Meta hashes IP itself), plus a SHA-256
  hashed country code.

This forwarding is off by default and only runs when the operator adds a
pixel. It is the highest-sensitivity processing quark does, because it is
the one path where visitor IP and User-Agent leave quark's boundary to a
third party. Enabling it, and disclosing it (or obtaining consent, depending
on your jurisdiction and how the data is used downstream), is the operator's
responsibility. `Sec-GPC: 1` suppresses this forwarding automatically, same
as analytics capture.

### Anonymizing the forwarded IP

By default the Meta forward sends the full client IP. Setting
`QUARK_PIXEL_ANONYMIZE_IP` (opt-in) truncates the IP before it leaves the
instance: an IPv4 address has its last octet zeroed (`a.b.c.d` becomes
`a.b.c.0`, a /24), and an IPv6 address keeps only its first 48 bits (a /48),
with the rest zeroed. An IP that cannot be parsed is omitted from the payload
rather than forwarded. The flag only affects the Meta `client_ip_address`
field; GA4 never receives an IP, and User-Agent is unchanged in this release.
Leaving the variable unset keeps the current behavior (full IP forwarded).

## Self-hosting and data residency

Because you run the quark binary and choose where its storage lives, quark
already answers "where does visitor data live and who processes it" without
any separate configuration: the answer is wherever you deployed it.

## What this release does not do

- No admin-panel UI for erasure; per-link erasure is API-only
  (`DELETE /admin/links/:code/analytics`).
- No fine-grained control over which fields Meta CAPI forwards; it is
  all-or-nothing per pixel, aside from the IP-anonymization flag above.
- No anonymization or omission of the forwarded User-Agent (a later phase);
  the IP is the field this release can anonymize.
- No cookie-consent banner. Given the cookieless design above, one is not
  needed for quark's built-in analytics; if you enable Meta or GA4
  forwarding, evaluate your own consent obligations for that specific
  activity.

These are tracked as follow-up work, not shipped in this release.
