# Proposed next features for quark (competitor-informed)

Date: 2026-07-14. Author: research pass on `research/competitors`.

This is a planning document, not an implementation. It reads the feature sets of
the leading commercial shorteners (short.io, Rebrandly, Bitly) plus the modern
open-source competitor (Dub.co), lists what quark does not yet have, and proposes
a prioritized set of next features. Every competitor claim is cited inline as a
plain URL with the date it was checked, because these pages change.

For what quark already ships, see [`ROADMAP.md`](../ROADMAP.md) and
[`ARCHITECTURE.md`](../ARCHITECTURE.md). Features already in quark (computed short
codes, custom aliases, TTL and max-visits expiry, tags, QR codes, geo/device
redirect rules, A/B variants, device-aware deep links with AASA/assetlinks, a UTM
builder, rich analytics with a ClickHouse option, signed webhooks plus
Slack/Discord/Telegram channels, server-side GA4/Meta CAPI forwarding, scoped API
tokens, CSV/JSON import, and multi-node scaling) are treated as done and are not
proposed again here.

## How the competitors were read

- short.io features and bulk tooling: https://short.io/features/ and
  https://docs.short.io/articles/features/bulk-features/how-to-shorten-or-create-links-in-bulk
  (checked 2026-07-14).
- Rebrandly features, import, and broken-link monitoring:
  https://www.rebrandly.com/features , https://support.rebrandly.com/en/articles/469544-rebrandly-s-guide-to-importing-links ,
  https://support.rebrandly.com/en/articles/469614-how-do-i-monitor-and-fix-broken-branded-links-404
  (checked 2026-07-14).
- Bitly features (links, QR, link-in-bio, threat scanning):
  https://bitly.com/pages/features/link-in-bio and
  https://bitly.com/pages/resources/press/bitly-connection-layer-links-qr-codes-2026/
  (checked 2026-07-14).
- Dub.co features, integrations, conversions, webhooks:
  https://dub.co/links , https://dub.co/help/category/link-management ,
  https://dub.co/integrations , https://dub.co/docs/conversions/quickstart
  (checked 2026-07-14).

## 1. Competitor feature matrix

"quark has it?" is checked against the current roadmap. "partial" means quark has
an overlapping capability but not the full feature the competitors sell.

| Feature | short.io | Rebrandly | Bitly | Dub | quark has it? |
|---|---|---|---|---|---|
| Google Sheets bulk create / sync | native "Bulk Sheets" tool | Google Sheets add-on | no (CSV) | no (API/Zapier) | no |
| Custom / branded domains | yes (10 to unlimited by plan) | yes (500+ TLDs) | yes (Growth tier) | yes | no (backlog) |
| Password-protected links | yes | yes | yes (paid) | yes (Pro) | no |
| Link cloaking / masking | yes | no (stated) | no | yes (Pro) | no |
| Expire by date with fallback redirect | yes (date or clicks) | yes | yes | yes (disable at date/time) | partial (410 only) |
| Broken-link / health monitoring | no | yes (30-day 404 monitor) | manual | no | no |
| QR customization / branding | yes | yes | yes (core product) | yes | partial (plain QR) |
| Link organization beyond tags (folders) | no | yes (workspaces) | campaigns | yes (folders) | partial (tags) |
| Retargeting pixels (FB/AdRoll/Criteo, client-side) | yes | yes | yes | no | partial (GA4/Meta server-side) |
| Revenue / conversion attribution (Stripe/Shopify) | no | no | no | yes | no |
| Native Zapier / Make / n8n app | via API | yes | yes | yes (7000+/2000+) | partial (webhooks feed them) |
| Real-time click / threshold alerts | no | no | no | webhooks | partial (per-event webhooks) |
| Link-in-bio / landing pages | no | no | yes | yes | no |
| Bulk operations UI (edit/delete many) | yes | yes | yes | yes (bulk actions) | partial (import + list) |
| Browser extension | yes (Chrome/Firefox) | yes | yes | yes | no |
| Team / workspace multi-user | yes | yes | yes | yes | no (cloud phase) |
| Deep linking / smart mobile routing | yes (Team) | yes | yes | yes (deferred deep links) | partial (device-aware) |
| Click-time threat / malware scanning | no | no | yes | no | partial (create-time blocklist) |
| GDPR / consent tooling | region targeting | yes (EU data) | yes | yes (EU hosting) | no |

Sources for the matrix rows are the competitor pages listed under "How the
competitors were read" above, all checked 2026-07-14.

## 2. Proposed features, prioritized

Each item states what it is, which competitors have it, why it matters, a rough
effort (S/M/L), and how it fits quark's shape: one binary, backends behind traits,
and a redirect hot path that must stay a decode plus one cache lookup.

### Tier 1: build these first

#### 2.1 Google Sheets mass import with live sync (owner callout)

What it is. short.io ships a native "Bulk Sheets" tool: from the dashboard you
grant short.io permission to create a Google spreadsheet, you fill an "Original
URL" column (optionally "Link slug" and "Tags"), click "Create links", and the
sheet's "Short URL" and "Status" columns are written back per row. New links land
in the dashboard next to the sheet
(https://docs.short.io/articles/features/bulk-features/how-to-shorten-or-create-links-in-bulk,
checked 2026-07-14). short.io also supports an ongoing sync so that editing a long
URL or adding a row creates a new short link
(https://help.short.io/en/articles/4065878-how-to-integrate-short-io-with-google-sheets-zapier,
checked 2026-07-14). Rebrandly has the same shape as a Google Workspace add-on,
capped near 1,000 links per run
(https://support.rebrandly.com/en/articles/469544-rebrandly-s-guide-to-importing-links,
checked 2026-07-14).

Who has it. short.io (native), Rebrandly (add-on).

Why it matters. This is the owner's explicit ask. quark already has the hard half:
`POST /admin/import` bulk-creates from CSV/JSON with a partial-success per-row
report ([`IMPORT.md`](../IMPORT.md)). A spreadsheet is just another row source that
also wants the created short URL written back. It is the lowest-friction bulk entry
point for non-technical operators, who live in spreadsheets, not curl.

Effort. M for the one-shot create-from-sheet path. L if quark hosts the live sync
itself (needs stored Google OAuth tokens, a Sheets API poll or push, and a
per-sheet cursor of already-processed rows).

Architecture fit. Keep the redirect path untouched; this is all `POST /`-side. Two
honest options, in increasing cost:

1. Thin path, no Google credentials in quark. Ship a published Google Apps Script
   (or a documented Sheets template) that reads the rows client-side and calls the
   existing import/create API with an API token, writing the returned short URL
   back into the sheet. quark stores nothing new. This matches quark's
   "single binary, no surprise dependencies" stance and is the recommended first
   cut.
2. Native sync inside quark. A new optional module holds a Google OAuth token per
   connected sheet and a `sheet_id -> last_processed_row` cursor in the store,
   polls on an interval, and creates links for new or changed rows. This is a real
   networked integration and should be opt-in and off by default, the same way
   Valkey/ClickHouse/Postgres are selected by env var. It does not belong in the
   zero-dependency core.

Recommendation: ship option 1 now (documented template plus a small write-back
script), keep option 2 as a follow-up gated behind config.

#### 2.2 Custom / branded domains management

What it is. Let an operator serve short links from their own domains and
subdomains (`go.acme.com/xyz`), managed in the panel, with per-domain settings.
short.io allots 10 to unlimited domains by plan (https://short.io/features/,
checked 2026-07-14); Rebrandly leans on 500+ TLDs
(https://www.rebrandly.com/features, checked 2026-07-14); Bitly and Dub both offer
branded domains (https://dub.co/links, checked 2026-07-14).

Who has it. All four.

Why it matters. Branded domains are the single most common paid upgrade across
these tools and the most requested thing a self-hoster wants after "it shortens
URLs". quark already lists it in the roadmap backlog; this is confirmation it
should move up.

Effort. M. Most of the weight is operational (TLS certificates and DNS), not code.

Architecture fit. quark is already deployed behind a proxy or CDN (see
[`EDGE.md`](../EDGE.md)), so the cleanest cut is: quark stays domain-agnostic on the
redirect and resolves a code the same way regardless of host, while a new
`domains` table records which hosts are allowed and their defaults (default
destination, 404 behavior). Certificate issuance stays at the proxy layer
(Caddy on-demand TLS, or Cloudflare) and is documented, not built into the binary.
If per-domain link namespaces are wanted (the same slug meaning different links on
two domains), that changes the key from `id` to `(domain_id, id)` and touches the
codec assumption that a code alone resolves a link; scope that carefully before
committing, since it complicates the hot path.

#### 2.3 Password-protected links

What it is. A link asks the visitor for a password before it redirects. Offered by
short.io (https://short.io/features/password/, checked 2026-07-14), Rebrandly,
Bitly, and Dub on Pro (https://dub.co/help/category/link-management, checked
2026-07-14).

Who has it. All four.

Why it matters. Common ask for gated content, internal links, and paid drops. It is
a small, self-contained feature with clear value.

Effort. S to M.

Architecture fit. This one does touch the hot path, so design it to stay cheap. Add
an optional `password_hash` to the `Record` (argon2 or bcrypt hash, never the
plaintext). The redirect checks a gate flag first, the same cached-atomic trick the
webhook `link.clicked` path already uses so unprotected links pay nothing. When a
link is protected, quark serves a small interstitial HTML form instead of a 302;
the POST checks the hash and, on success, issues the redirect and sets a
short-lived signed cookie so repeat visits skip the prompt. The password check is
the only case where quark returns a body on the redirect route, so keep that branch
behind the flag.

#### 2.4 Link cloaking / masking

What it is. Keep the short URL visible in the address bar while the destination
loads, by rendering the target inside a frame instead of a 302. short.io sells this
for privacy and aesthetics (https://short.io/features/cloaking/, checked
2026-07-14); Dub offers it on Pro (https://dub.co/help/article/link-cloaking,
checked 2026-07-14). Rebrandly explicitly does not offer cloaking
(https://linklyhq.com/review/rebrandly, checked 2026-07-14).

Who has it. short.io, Dub.

Why it matters. Requested for affiliate links and campaigns where the operator does
not want the destination visible. It is worth flagging the tradeoffs: framing
breaks destinations that set `X-Frame-Options`/CSP `frame-ancestors`, hurts SEO,
and is often used to obscure where a link goes, which cuts against quark's abuse
posture. Propose it as opt-in, off by default, and documented with its limits.

Effort. S.

Architecture fit. A cloaked link returns a tiny HTML page with a full-page iframe
(or a meta-refresh) rather than a 302. Same gate-flag pattern as password links so
the common case stays a pure redirect. Analytics capture stays identical since the
click still hits the redirect handler.

#### 2.5 Expire-with-redirect (fallback destination on expiry)

What it is. Today a quark link past its TTL or max-visits returns `410 Gone`
([`ARCHITECTURE.md`](../ARCHITECTURE.md), "Destination precedence"). Competitors let
an expired link instead redirect somewhere: an archive page, a "campaign ended"
page, or a new URL. Dub disables at a date/time (https://dub.co/links, checked
2026-07-14); short.io expires by date or click count
(https://blog.short.io/link-expiry-link-cloaking-and-password-protection-advanced-features-you-need-to-protect-your-links/,
checked 2026-07-14).

Who has it. short.io, Rebrandly, Bitly, Dub.

Why it matters. `410 Gone` is a dead end for a visitor. A fallback keeps the traffic
and is a near-free extension of machinery quark already has.

Effort. S.

Architecture fit. Add an optional `expired_url: Option<String>` to the `Record`. The
existing expiry check already runs at read time; when it fires and `expired_url` is
set, return a 302 to it instead of 410. No new storage shape, no hot-path cost for
links without it. Run the fallback URL through the same SSRF/blocklist checks as
rules and variants so it cannot smuggle an internal host.

#### 2.6 Broken-link / link-health monitoring

What it is. Watch destinations and flag links whose target returns an error, so the
operator can repoint them before visitors hit a 404. Rebrandly monitors clicked
links over a rolling 30 days and surfaces broken ones for one-click repair
(https://support.rebrandly.com/en/articles/469614-how-do-i-monitor-and-fix-broken-branded-links-404,
checked 2026-07-14).

Who has it. Rebrandly (built-in). Bitly treats it as manual review.

Why it matters. A short link outliving its destination is the most common silent
failure in link management. quark already has the delivery machinery (webhooks and
Slack/Discord/Telegram channels) to alert on it, so the missing piece is the
checker, not the notification.

Effort. M.

Architecture fit. A new optional background worker, in the same family as the
analytics worker and the webhook relay, periodically issues a `HEAD`/`GET` to the
destinations of active links (rate-limited, respecting the SSRF guard) and records
last-checked status. On a transition to unhealthy it emits a `link.unhealthy`
event through the existing webhook/channel path. Off by default and never on the
redirect path. On Postgres it can lease work with `FOR UPDATE SKIP LOCKED` the way
the outbox relay already does, so multiple nodes share the scan without duplicating
it.

### Tier 2: strong, after Tier 1

#### 2.7 Bulk operations UI

What it is. Select many links in the panel and edit, tag, move, or delete them in
one action. Dub ships bulk link actions
(https://dub.co/help/category/link-management, checked 2026-07-14); short.io,
Rebrandly, and Bitly all have bulk management.

Who has it. All four.

Why it matters. quark has bulk create (import) but no bulk edit or delete. As a
link set grows this is the difference between usable and painful.

Effort. M, mostly frontend plus a batch admin endpoint.

Architecture fit. A `POST /admin/links/bulk` that takes a set of codes and an
operation, reusing the existing per-link mutation paths and cross-node
invalidation. No hot-path impact.

#### 2.8 QR code customization and branding

What it is. quark generates plain QR codes today (roadmap "Done", panel). Bitly
makes branded, customizable QR codes a core product
(https://bitly.com/pages/resources/press/bitly-connection-layer-links-qr-codes-2026/,
checked 2026-07-14); Rebrandly, short.io, and Dub all offer QR styling.

Who has it. All four.

Why it matters. Colors, a center logo, custom error-correction, and export formats
(SVG/PNG/PDF) are table stakes for print and campaign use.

Effort. S to M, mostly in the SPA / QR library.

Architecture fit. Pure presentation. Generation already happens client-side or at a
QR endpoint; adding style parameters does not touch the store or the redirect path.

#### 2.9 Folders for link organization

What it is. Group links into folders, on top of the existing free-form tags. Dub
has folders on Pro (https://dub.co/help/category/link-management, checked
2026-07-14).

Who has it. Dub (folders); short.io, Rebrandly, Bitly use workspaces/campaigns for
the same job.

Why it matters. Tags are flat and many-to-many; folders give a single hierarchy
that some operators prefer for large sets. quark already has the tag machinery, so
this is an organizational addition, not a new data spine.

Effort. S to M.

Architecture fit. An optional `folder` field on the `Record` plus a list filter,
mirroring how `tag` filtering already works (`GET /admin/links?tag=`). No hot-path
impact.

#### 2.10 Client-side retargeting pixels beyond GA4/Meta

What it is. quark already forwards conversions server-side to GA4 and Meta CAPI
([`CONVERSION-FORWARDING.md`](../CONVERSION-FORWARDING.md)). The competitors also
fire client-side retargeting pixels from the link so a click adds the visitor to ad
audiences on Facebook, Google Ads, Twitter, AdRoll, and Criteo, even without a
visit to the operator's own site
(https://linklyhq.com/review/rebrandly, checked 2026-07-14).

Who has it. Rebrandly, short.io, Bitly.

Why it matters. Server-side CAPI and client-side retargeting pixels do different
jobs; retargeting audience-building needs the pixel to fire in the visitor's
browser, which quark's server-side forwarding does not do.

Effort. M, and it needs a design decision (see open questions). Firing a pixel in
the browser means an interstitial or an injected script, which conflicts with the
clean 302. This trades hot-path purity for the feature and should be opt-in per
link only.

Architecture fit. This is the one Tier 2 item that fights quark's redirect model.
If pursued, gate it exactly like cloaking: an opt-in link type that returns a small
HTML page carrying the pixel and then forwards, so unmarked links keep the pure 302.

#### 2.11 Revenue and conversion attribution (Stripe / Shopify)

What it is. Dub's headline differentiator: tie a click to a downstream sale via
Stripe or Shopify and attribute revenue to the link
(https://dub.co/docs/conversions/quickstart, checked 2026-07-14).

Who has it. Dub (the others do clicks, not revenue).

Why it matters. This is where Dub is pulling ahead of the classic shorteners. It is
a larger surface (a lead/customer/sale event model, dedup, and an ingestion API)
but it reuses quark's existing click-event pipeline and its GA4/Meta dedup ids.

Effort. L.

Architecture fit. Extend the analytics event model with conversion events keyed by
the same per-click dedup id quark already generates for pixel forwarding, ingested
through a new `POST` webhook receiver (Stripe/Shopify webhooks in) rather than on
the redirect. Best suited to the Postgres/ClickHouse backends; the embedded sink
can keep a simpler count. Keep it entirely off the hot path.

#### 2.12 Native automation apps plus real-time click alerts

What it is. quark's signed webhooks and chat channels already feed automation, but
the competitors ship first-party Zapier/Make/n8n apps and packaged alerts. Dub
connects to 7000+ apps via Zapier and 2000+ via Make and shipped real-time
webhooks (https://dub.co/integrations, checked 2026-07-14;
https://dub.co/blog/introducing-webhooks, checked 2026-07-14).

Who has it. Dub (native apps), Rebrandly and Bitly (integrations).

Why it matters. quark's webhooks are the primitive; a published Zapier/n8n app and a
few packaged recipes (including a threshold alert like "ping this channel when a
link passes N clicks in an hour") turn the primitive into something a
non-developer can wire up. The memory note about n8n on the quark roadmap points
the same way.

Effort. S for a published n8n/Zapier template on top of existing webhooks; M for a
server-side threshold-alert rule (needs a small counter-window check off the hot
path, in the analytics worker).

Architecture fit. Templates are documentation. The threshold alert lives in the
analytics worker, which already sees every click, and emits through the existing
channel delivery path. Nothing new on the redirect.

#### 2.13 Link-in-bio / simple landing pages

What it is. A hosted page that lists several links behind one URL. Bitly and Dub
both ship it (https://bitly.com/pages/features/link-in-bio, checked 2026-07-14;
https://dub.co/links, checked 2026-07-14).

Who has it. Bitly, Dub (and the Linktree category generally).

Why it matters. It is a common creator/marketing ask and a natural upsell, but it is
a content-management and page-rendering surface that is a real step outside "URL
shortener". Propose it as a clearly-scoped optional module, not core.

Effort. L.

Architecture fit. This is the least aligned with quark's current shape. A bio page
is a stored document plus a rendered HTML route; it does not reuse the codec or the
redirect path and it drags in templating and asset handling. If pursued, isolate it
behind its own module and route prefix, keep it opt-in, and treat it as its own
mini-product rather than a link property. Lowest priority of the Tier 2 set for
that reason.

### Tier 3: noted, deprioritized or already placed

- Team / workspace multi-user. Already the roadmap's declared cloud-phase,
  proprietary feature; OSS stays single-operator. No change proposed.
- Browser extension. All four ship one
  (https://short.io/features/tools-extensions/, checked 2026-07-14). It is a
  separate client that just calls the existing create API with a token; worth doing
  eventually but it is a client project, not a change to the binary. S to M, low
  urgency.
- Click-time threat/malware scanning. Bitly re-checks URLs at click for
  phishing/malware (https://bitly.com/pages/resources/press/bitly-connection-layer-links-qr-codes-2026/,
  checked 2026-07-14). quark checks the destination against a blocklist at create
  time only. A click-time scan would tax the hot path and needs a threat feed;
  leave as create-time plus the health monitor (2.6) unless a strong case appears.
- GDPR / consent tooling. The competitors lean on this for EU customers. quark's
  self-host story already helps here (the operator controls the data and the
  region), and the bot filter plus IP handling are in place. A cookie/consent mode
  for the analytics and any interstitials is a smaller follow-up once interstitials
  (password/cloaking/pixels) exist.

## 3. quark's differentiators to keep

The roadmap should add features without eroding what makes quark quark. Hold these
lines:

- Computed short codes. Codes are a pure function of the id and the key via the
  calibrated Feistel/ARX permutation; there is no `code -> id` table and no
  collision check on create ([`ARCHITECTURE.md`](../ARCHITECTURE.md)). Custom
  domains with per-domain namespaces (2.2) is the one proposal that could pressure
  this; keep a single global code space unless there is a hard reason not to.
- Single binary, zero-dependency core. Network backends are opt-in by env var,
  chosen at startup, with no build-time feature flags. Every proposal above that
  needs a network dependency (Google Sheets live sync, Stripe/Shopify attribution)
  must stay opt-in and off by default, the way Postgres/Valkey/ClickHouse already
  are.
- The redirect hot path stays a decode plus one cache lookup. Password links,
  cloaking, and browser pixels all return a body instead of a 302; each must sit
  behind a cached gate flag so an ordinary link pays nothing, the same pattern the
  `link.clicked` webhook already uses.
- Self-host and OSS (AGPL-3.0). The proposals here are all things a single operator
  can run on their own box. Multi-tenant accounts stay the separate cloud edition;
  do not let a "team" feature pull tenancy into the OSS core.

## 4. Open questions for the owner

1. Google Sheets: ship the thin documented-template path first (no Google
   credentials in quark, script writes short URLs back), or go straight to native
   in-quark live sync with stored OAuth tokens? The first keeps the core clean; the
   second matches short.io's built-in feel.
2. Custom domains: single global code space across all domains (simpler, keeps the
   codec assumption), or per-domain namespaces (more flexible, complicates the hot
   path and key shape)? This decision gates the effort estimate for 2.2.
3. Interstitial policy: password protection, cloaking, and client-side retargeting
   pixels all break the pure 302. Is returning an HTML interstitial on opt-in links
   acceptable, given the hot-path stance? If yes, one shared interstitial mechanism
   can serve all three.
4. Link cloaking is often used to hide destinations and sits awkwardly with quark's
   abuse posture. Ship it (opt-in, documented limits) or decline it on principle?
5. How far toward attribution should the OSS edition go? Revenue tracking (2.11) is
   Dub's moat and a big surface. Is that in scope for self-hosted quark, or is it a
   cloud-edition differentiator?
6. Retargeting pixels (2.10): worth trading a clean redirect for, or leave
   audience-building to the server-side CAPI forwarding quark already has?

## 5. Decisões do dono (2026-07-14)

As perguntas da seção 4 foram resolvidas assim:

- **Links com senha (2.3): SIM.** Constrói o mecanismo de interstitial opt-in (uma
  página HTML pequena, atrás de um gate cacheado) só para os links marcados; o
  redirect comum segue 302 puro sem pagar nada.
- **Expira-com-fallback (2.5): SIM.** Ao expirar, em vez de 410, faz 302 para o
  destino de fallback. É redirect puro, não usa o interstitial.
- **Cloaking (2.4): NÃO.** Recusado por princípio: esconder o destino final encaixa
  mal com a postura anti-abuso do quark.
- **Pixel de retargeting no browser (2.10): NÃO.** O encaminhamento server-side por
  CAPI (GA4/Meta) que o quark já tem cobre a necessidade sem quebrar o 302.
- **Google Sheets (2.1): NATIVO com OAuth.** Sync ao vivo dentro do quark (linha
  nova vira link novo), com os tokens OAuth guardados. Opt-in e desligado por
  padrão, como os outros backends de rede.
- **Domínios customizados (2.2): ESPAÇO DE CÓDIGO GLOBAL.** Um único espaço de
  códigos para todos os domínios, preservando a suposição do codec e o hot path.
- **Atribuição de receita (2.11): FORA.** Não entra no quark por ora, nem OSS nem
  como prioridade; se um dia existir, é diferencial da edição cloud.

Backlog resultante, do mais barato ao mais estrutural: expira-com-fallback (S) →
links com senha + o mecanismo de interstitial (S-M) → monitoramento de link
quebrado (2.6, M, sem decisão pendente) → domínios customizados globais (2.2, M) →
Google Sheets nativo com OAuth (2.1, L).
