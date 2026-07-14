**English** · [Português](CONVERSION-FORWARDING.PT_BR.md)

# Conversion forwarding (roadmap #14)

## What it does

quark can forward a conversion event to GA4 and/or Meta on each click,
server-side, with no tracker on the client. There is no pixel script, no
cookie set in the visitor's browser, and no third-party request made from
their device at all. The redirect page never talks to Google or Meta; quark
does, from its own backend, after the fact.

This is the opposite of how conversion tracking usually works. A normal
pixel loads in the visitor's browser and phones home to the ad platform
directly, which is what ad blockers, Safari's ITP, and privacy-conscious
users block. Forwarding the same event from the server sidesteps all of
that, at the cost of losing whatever client-side signal (cookies, device
fingerprint) a real pixel would have captured. What quark sends is coarser:
a link code, a country, and a timestamp. Good enough to tell a provider "a
click on this link happened," not enough to build a profile.

## The two providers

- **GA4** (Google Analytics 4), via the **Measurement Protocol**.
- **Meta CAPI** (Meta Conversions API), for Facebook/Instagram ads.

Both are **instance-level** configuration for this pass: one or more pixel
configs live under the operator's account, and every click forwards to each
one that is active. There is no per-link targeting yet (see the follow-up
note at the bottom).

### Getting a GA4 Measurement Protocol API secret

1. In GA4, go to **Admin > Data Streams**, pick the stream for your property.
2. Under that stream, open **Measurement Protocol API secrets**.
3. Create a new secret. Copy it, along with the stream's **Measurement ID**
   (the `G-XXXXXXXXXX` value shown at the top of the stream page).
4. Enter both in quark's Pixels page: Measurement ID + API secret.

### Getting a Meta Conversions API access token

1. In **Events Manager**, select (or create) the pixel you want to forward
   to. Note its **Pixel ID** (a numeric id, shown on the pixel's overview
   page).
2. Under that pixel's settings, go to **Conversions API** and generate an
   **access token** (or use a System User token from Business Settings if
   you want one that doesn't expire on a personal account change).
3. Enter both in quark's Pixels page: Pixel ID + access token.

## Privacy posture

Only fields quark already captures for the click analytics feature are
forwarded:

- the link's short code (not the internal id),
- the click's country (already derived server-side, e.g. from a CDN
  geo-header),
- the event's timestamp.

**Not sent**: the visitor's IP address, their raw User-Agent string, or any
other client-side identifier. GA4 receives a synthetic `client_id`
(generated per quark instance, not per visitor) instead of a real user id;
Meta receives no user-data hashing pass in this version, so treat what
arrives at either provider as an **anonymous conversion ping**, not an
attributable user event. Advanced matching (hashed email/phone) is
explicitly out of scope for now.

## Async, fail-open, never on the hot path

Forwarding runs from quark's existing **analytics worker**, the same
background path that already writes click events to the analytics sink. It
does **not** run inline with the redirect: a click gets its 302 response
immediately, regardless of whether GA4 or Meta are configured, reachable, or
slow. The worker batches clicks and forwards each batch to every active
pixel config after the fact.

This is also **fail-open**: if a provider is down, rate-limiting quark, or
returns an error, that failure is logged and dropped. It never affects the
redirect, never blocks the worker's next batch, and never surfaces to the
end user. There is no retry queue; a batch that fails to forward is not
retried. If a provider is down for an extended period, the conversions for
that window are simply lost, not backfilled. That's a deliberate simplicity
tradeoff for this pass; see the follow-up note below.

## No SSRF surface

The provider hosts (`https://www.google-analytics.com` for GA4,
`https://graph.facebook.com` for Meta) are fixed in code. The operator
supplies credentials (Measurement ID/API secret, Pixel ID/access token), not
URLs, through the Pixels page. There is no field anywhere that lets an
operator (or an attacker who compromises the panel) point quark's outbound
requests at an arbitrary host. The base host is only injectable in test
code, never from the API or the UI.

## Managing pixels

The **Pixels** page in the web panel (`/pixels`) lists configured pixels,
lets you add one (pick a provider, then fill in that provider's two
credential fields), and remove one. Secrets (`api_secret`, `access_token`)
are masked as `••••` once saved; the identifiers (`measurement_id`,
`pixel_id`) are shown in clear since they aren't secrets on their own. All
of this sits behind the same `x-admin-token` used for the rest of the
panel; there is no separate permission for pixels.

## Follow-ups (not in this pass)

- Additional providers (GTM, TikTok, LinkedIn) once this pattern is proven.
- Per-link targeting of pixels (today it's all-active-pixels, every click).
- Advanced matching / user-data hashing.
- Retry or durability beyond the worker's current best-effort delivery.
