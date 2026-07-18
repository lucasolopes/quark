# LUC-44: GDPR / consent tooling — what quark actually needs

Date: 2026-07-18. Author: research/design pass for LUC-44.

This is a research and design-recommendation document, not an implementation.
It answers one question before any code is written: given how quark actually
handles visitor data today, what does a "GDPR / consent tool" for quark need to
be? The short version is that quark's analytics are server-side and cookieless,
which removes the usual reason for a consent banner, so LUC-44 should be a
smaller, honest set of operator controls rather than a cookie-consent widget.

Every claim about quark's behavior below is grounded in a file:line reference so
it can be re-checked. Every legal claim is cited inline with the date it was
checked, because this area moves.

## 1. Findings: what quark does with visitor data today

### 1.1 The redirect hot path sets no cookie on the visitor

The normal redirect (`GET /:code`) resolves the destination, records a click,
and returns a `302` with exactly two headers: `Location` and `Cache-Control`.
No `Set-Cookie`, no client-side script, no third-party request from the
visitor's browser (`src/api.rs:1360`-`1367`). A visitor who clicks a normal
short link has nothing written to or read from their device.

### 1.2 Analytics are captured server-side from request headers

At redirect time quark builds a `ClickEvent` from data already present on the
request (`src/api.rs:1315`-`1338`):

- `country` and `city` from CDN geo headers (`cf-ipcountry`, `cf-ipcity`),
  `src/api.rs:1261`-`1264`, `1325`-`1328`.
- `user_agent` from the `User-Agent` header, `src/api.rs:1265`-`1268`.
- `referer` from the `Referer` header, `src/api.rs:1319`-`1322`.
- `ip` from the configured real-IP header (default `CF-Connecting-IP`) or the
  socket, `src/api.rs:1330`-`1333`, `client_ip` at `src/api.rs:748`-`765`.
- `fbc` derived from the `fbclid` query param, `src/api.rs:1334`-`1335`.

The event is handed to an in-process channel (`analytics_tx.try_send`,
`src/api.rs:1358`) and processed off the hot path by the analytics worker
(`src/analytics/mod.rs:365`). This matches the claim in
`docs/CONVERSION-FORWARDING.md:5`-`11`: "server-side, with no tracker on the
client ... no cookie set in the visitor's browser." Confirmed in code.

### 1.3 IP and fbc never touch disk; other fields do (bounded retention)

`ip` and `fbc` on `ClickEvent` are `#[serde(skip)]`
(`src/analytics/mod.rs:37`-`40`). They live in memory only for the lifetime of
the conversion forward and are never serialized into the persisted recent-events
buffer. Two unit tests enforce this: `serialized_clickevent_never_contains_ip_or_fbc`
(`src/analytics/mod.rs:638`) and `old_clickevent_json_without_ip_fbc_deserializes_with_none`
(`src/analytics/mod.rs:624`). The doc's "IP and fbc never touch disk" claim
(`docs/CONVERSION-FORWARDING.md:85`-`93`) is accurate.

What does persist: `user_agent`, `referer`, `country`, `city`, `ts`, plus the
per-click `event_id` and derived aggregates. Retention is bounded — the raw
recent-events buffer is capped per link at `EVENTS_MAX = 1000`
(`src/analytics/mod.rs:171`) and trimmed circularly in both backends
(`src/store/lmdb.rs:1201`-`1202`, `src/store/postgres.rs:2908`,`2976`). There is
no time-based retention window and no per-visitor deletion path today.

Aggregates keep only coarse, low-cardinality breakdowns (country, city, device,
OS, browser, referer host, per-day counts) — `src/analytics/mod.rs:53`-`78`.
Referers are reduced to hostname to bound cardinality
(`referer_host`, `src/analytics/mod.rs:292`). Bots are detected by UA heuristic
and excluded from every breakdown (`is_bot`, `src/analytics/mod.rs:257`).

### 1.4 IP is also used for rate limiting and geo — a functional use

`client_ip` feeds the per-IP rate limiter on the password submit and other
state-changing endpoints (`src/api.rs:586`, `1087`, and the `abuse/ratelimit`
path). This is a security/abuse-prevention use of the IP, independent of
analytics, and is the kind of processing regulators treat as strictly necessary.

### 1.5 What quark sends off-box (conversion forwarding)

When an operator configures a GA4 or Meta pixel, the analytics worker forwards
events server-side (`src/analytics/mod.rs:476`-`508`). GA4 receives only short
code, country, timestamp, and a per-instance synthetic `client_id` — an
anonymous ping (`docs/CONVERSION-FORWARDING.md:57`-`69`). Meta CAPI additionally
carries `client_ip_address`, `client_user_agent`, and `fbc` in plaintext plus a
SHA-256 country (`docs/CONVERSION-FORWARDING.md:70`-`83`). This forwarding is
off by default; it exists only when the operator adds a pixel. It is the one
place raw visitor IP/UA leave quark's boundary, and it is the highest-sensitivity
processing in the system.

### 1.6 The one visitor-facing cookie: the password unlock cookie

The only cookie ever set on a visitor's browser is `qk_pw_<code>`, set after a
visitor submits the correct password for a protected link
(`src/api.rs:1158`-`1161`, `docs/LINK-PASSWORD.md:22`-`27`). It is `HttpOnly`,
`SameSite=Lax`, `Secure` over HTTPS, HMAC-signed, scoped to one link, and
expires in 12 hours (`crate::password::UNLOCK_TTL_SECS`). It carries no
identifier and no cross-site value; it only remembers "this browser already
entered the password for this link." This is a textbook strictly-necessary /
user-requested-service cookie (see 2.2).

### 1.7 Admin panel cookies are out of scope

The OIDC login session cookie (`SESSION_COOKIE`), the login-state cookie
(`LOGIN_COOKIE`, `src/api.rs:1616`+), and the Sheets OAuth state cookie
(`SHEETS_STATE_COOKIE`, `src/api.rs:3289`+) are all first-party authentication
cookies for the operator's own admin session. They are strictly necessary and
sit entirely outside visitor-consent scope.

### 1.8 There is no consent, DNT, or GPC handling anywhere today

A repo-wide search for `consent`, `GDPR`, `Sec-GPC`, `DNT`, `opt-out`,
`retention`, `anonymize`, `erasure` found only incidental matches (OAuth
scopes, doc prose, the "GDPR / consent tooling" row in
`docs/research/2026-07-14-next-features.md:63`,`408`-`412`). There is no code
that reads `Sec-GPC`/`DNT`, no opt-out, no configurable anonymization, no
retention policy, and no data-subject deletion path. LUC-44 is greenfield.

## 2. Analysis: what actually needs consent vs what is exempt

### 2.1 The cookie-banner trigger is not present for analytics

Under the ePrivacy Directive (Art. 5(3)), consent is required to store or read
information on a user's device; strictly necessary cookies are the only exempt
category, and everything else (including analytics cookies) needs prior consent
([GDPR.eu](https://gdpr.eu/cookies/), checked 2026-07-18;
[iubenda](https://www.iubenda.com/en/help/5525-cookies-gdpr-requirements/),
checked 2026-07-18). The trigger is the storage/read on the device, not the
analytics themselves.

quark's analytics store nothing on and read nothing from the visitor's device
(section 1.1-1.2). So the ePrivacy cookie-consent trigger simply does not fire
for quark's analytics. Industry and DPA guidance is consistent that truly
cookieless, no-persistent-identifier analytics do not require a consent banner;
CNIL and the German DSK have exempted properly configured privacy-first
analytics from consent
([CookieChimp](https://cookiechimp.com/blog/do-analytics-cookies-require-consent),
checked 2026-07-18;
[Secure Privacy](https://secureprivacy.ai/blog/server-side-consent-mode-for-ga4-how-to-track-analytics-while-respecting-privacy),
checked 2026-07-18).

Honest conclusion: **a cookie-consent banner for quark's built-in analytics is
unnecessary in the current architecture.** Building one would be
compliance theater and would hurt the product (a banner on a bare redirect makes
no sense — there is no page to show it on).

### 2.2 The password cookie is very likely exempt

The `qk_pw_<code>` cookie is set only in direct response to a user action (the
visitor submitting the password) and does only what that action asked for
(remember the unlock). That is the classic "strictly necessary / explicitly
requested service" exemption — the same bucket as a login/session or a
shopping-cart cookie
([CookieYes](https://www.cookieyes.com/blog/cookie-consent-exemption-for-strictly-necessary-cookies/),
checked 2026-07-18). No consent prompt is required to set it. It should still be
named in the operator's privacy notice.

### 2.3 GDPR still applies even without a banner

No cookie banner does not mean no obligations. IP and User-Agent are personal
data, and GDPR still governs server-side processing of them: the operator needs
a lawful basis (legitimate interest is generally available for privacy-respecting
server-side analytics) and a transparent privacy-notice entry, but not a banner
([taggrs](https://taggrs.io/server-side-tracking/gdpr/), checked 2026-07-18).
The genuinely sensitive processing is conversion forwarding (section 1.5), where
raw IP/UA are sent to Meta — that is a data-sharing-with-a-third-party
activity the operator must disclose and, depending on jurisdiction and how the
operator uses it for ad targeting, may well require actual consent. quark can
give the operator the switch; the operator owns the legal call.

### 2.4 quark's self-host story is already a privacy feature

Because the operator runs the binary and controls storage and region, quark
already answers the "where does the data live / who processes it" questions that
competitors sell as GDPR features
(`docs/research/2026-07-14-next-features.md:408`-`410`). What is missing is not
architecture; it is operator-facing controls and documentation.

## 3. Recommendation: a minimal, useful LUC-44

Frame LUC-44 not as "add a cookie banner" but as **"give the operator the
controls and documentation to run quark compliantly, and respect legally
recognized opt-out signals."** Concretely, in rough priority order:

### 3.1 Respect a universal opt-out signal for analytics (new)

Read `Sec-GPC: 1` (Global Privacy Control) on the redirect request and, when
present, suppress persistent analytics capture for that click — still serve the
redirect, still do strictly-necessary work (rate limiting), but skip writing the
click to analytics and skip conversion forwarding. GPC, unlike DNT, has real
legal standing in a growing list of US states and is defensible as an
opt-out-honoring signal under GDPR
([Pandectes](https://pandectes.io/blog/global-privacy-control-vs-do-not-track-whats-legally-enforceable-in-2026/),
checked 2026-07-18). DNT is effectively dead (Firefox removed it Feb 2025) and
is at most a cheap extra check to honor alongside GPC, not the primary signal.
This is the single most credible, low-cost "privacy tool" quark can ship, and it
is honest: it changes real behavior rather than showing a banner.

### 3.2 Configurable IP / geo handling for analytics (partly exists)

- Already true: IP is never persisted for analytics (section 1.3). Document this
  loudly; it is a selling point.
- New: an operator setting to disable conversion forwarding of raw IP/UA, or to
  coarsen it (e.g. drop `client_ip_address`/`client_user_agent` from Meta
  payloads), for operators who cannot justify sharing it. Today forwarding is
  all-or-nothing per pixel.
- New (optional): a switch to drop or truncate `city` (finer-grained geo) while
  keeping `country`, for operators who want coarser location data.

### 3.3 Configurable analytics retention + a purge path (new)

Today retention is only the circular `EVENTS_MAX = 1000` cap (section 1.3). Add:

- An operator-configurable time-based retention for the raw recent-events buffer
  (e.g. drop events older than N days), keeping aggregates.
- A documented purge/delete operation the operator can run to satisfy an erasure
  request. Note that quark's data is already non-identifying at rest (no IP), so
  the realistic granularity is "purge a link's events" rather than "delete one
  visitor," and that should be stated plainly.

### 3.4 An operator compliance doc (new, cheap, high value)

Write `docs/PRIVACY.md` (+ `.PT_BR.md` twin per project convention) that states,
with the file:line-level honesty of this research: what quark collects, that
analytics are cookieless and IP is never persisted, what the one visitor cookie
is and why it is exempt, what conversion forwarding sends to Meta/GA4 and that
enabling it is the operator's disclosure/consent responsibility, and a template
privacy-notice paragraph the operator can paste. This is the highest
value-per-effort item and unblocks the "GDPR tooling" checkbox competitors sell.

### 3.5 Consent only if/when an interstitial changes the model (future, gated)

If quark ever adds an interstitial that sets a non-functional cookie (e.g. an
ad-supported interstitial, a cross-link visitor identifier, or client-side pixel
loading), *then* a consent step becomes genuinely required. Today none of the
interstitials do this (the password cookie is functional). Recommendation: do
not build a consent-banner framework speculatively; gate it on that concrete
feature arriving.

### What already exists vs what is new

- Exists: cookieless server-side analytics; IP/fbc never persisted; bounded
  per-link retention; bot exclusion; self-host data residency; functional-only
  visitor cookie.
- New in LUC-44: GPC (and cheap DNT) opt-out on the redirect; forwarding-level
  IP/UA controls; configurable time-based retention + purge/erasure path;
  `docs/PRIVACY.md`; (deferred) a real consent step gated on a future
  non-functional interstitial.

## 4. Open decisions for the owner

1. **Scope of LUC-44 v1.** Is v1 just GPC-honoring + `docs/PRIVACY.md` (small,
   shippable fast), or does it also include retention config and
   forwarding-level IP controls? Recommendation: v1 = GPC + doc; v2 = retention +
   forwarding controls.
2. **GPC default.** Honor `Sec-GPC` by default, or make it an operator opt-in
   flag? Recommendation: default-on, with an env flag to disable, so
   privacy-respecting behavior is the default.
3. **What GPC suppresses.** Analytics only, or analytics + conversion forwarding?
   Recommendation: both — forwarding to Meta is exactly the activity a
   sale/share opt-out is meant to stop.
4. **Retention model.** Time-based drop of raw events only, or also aggregate
   rollup/expiry? Aggregates carry no PII, so probably keep them indefinitely.
5. **Erasure granularity.** Is per-link purge sufficient given there is no
   per-visitor identifier at rest, or does any target market need per-IP deletion
   (which would require persisting IP — a step backwards)? Recommendation: per-link
   purge only; do not start persisting IP.
6. **Conversion-forwarding consent posture.** Does quark take any position on the
   operator's duty to obtain consent before enabling Meta forwarding, or is that
   purely documented as the operator's responsibility? Recommendation: document
   it as the operator's responsibility and provide the off switch.
