# quark cloud pricing plans (market-informed proposal)

Date: 2026-07-18. Author: product/pricing research pass.

This is a planning document, not a commitment. It reads the public pricing of the
leading URL shorteners, extracts the pattern they all converge on, maps that
pattern onto features quark actually ships (per `ROADMAP.md` and
`ARCHITECTURE.md`), and proposes a tier structure for the paid **quark cloud**
(the multi-tenant SaaS). The self-host binary stays AGPL-3.0 and free; only the
cloud is billed. Every competitor number is cited inline with the date checked,
because these pages change often.

Related: multi-tenancy design specs `docs/specs/2026-07-16-multi-tenancy-*` (the
cloud data model, roles `Owner/Admin/Member/Viewer`, custom domains P3), and the
prior feature research `docs/research/2026-07-14-next-features.md`. Billing itself
is tracked as LUC-41; this doc feeds that.

## 1. Market pattern

### 1.1 Comparative table

Prices are USD, monthly unless noted; "annual" is the effective per-month price on
an annual commitment. "Tracked" is the analytics metering unit each vendor uses.

| Vendor | Tiers | Entry paid (mo / annual) | Metering unit | Free plan | Enterprise gate |
|---|---|---|---|---|---|
| Dub.co | 5: Free, Pro, Business, Advanced, Enterprise | Pro $25 | tracked **events** (not clicks) | 25 links/mo, 1k events, 30d retention, 3 domains, 1 user | SAML/SSO, audit logs, SLA, unlimited everything |
| Bitly | 5: Free, Core, Growth, Premium, Enterprise | Core $10 (annual only) | **links/mo** + QR/mo; clicks unlimited | 5 links/mo, 2 QR/mo, no history | SSO, 99.9% SLA, webhooks, multi-user, custom limits |
| Short.io | 5: Free, Hobby, Pro, Team, Enterprise | Hobby $5 ($60/yr) | branded **links** (total) + tracked **clicks/mo** | 1k links, 50k clicks/mo, 5 domains, 1 user | SSO, 99.9% SLA at Team+; unlimited domains |
| Rebrandly | 5: Free, Essentials, Professional, Growth, Enterprise | Essentials $11 ($8/yr) | branded **links/mo** + tracked **clicks/mo** | 10 links/mo, 100 clicks, 1 domain, 1 user | SSO (SAML), 99.99% SLA, custom volumes |
| TinyURL | 4: Free, Pro, Bulk, Enterprise | Pro ~$10-16 | **active URLs** + API calls/mo | basic shorten + QR | sales-led |

Sources (all checked 2026-07-18):
- Dub: https://dub.co/pricing (Business $90 / Advanced $300 / Enterprise custom captured on-page; Free $0 = 25 links/mo, 1k events, 30d retention, 3 domains, 1 user, and Pro $25 = 1k links/mo, 50k events, 1yr retention per https://dub.co/pricing and the linked pricing history https://dub.co/blog/new-pricing).
- Bitly: https://bitly.com/pages/pricing (Free $0, Core $10/annual, Growth $29 annual / $35 mo, Premium $199 annual / $300 mo, Enterprise custom).
- Short.io: https://short.io/pricing/ (Free $0, Hobby $5, Pro $18, Team $48, Enterprise $148).
- Rebrandly: https://www.rebrandly.com/pricing (Free $0, Essentials $8-11, Professional $22-32, Growth $69-99, Enterprise custom).
- TinyURL: https://tinyurl.com/app/pricing plus third-party summaries (Free, Pro, Bulk, Enterprise), values vary by source.

### 1.2 What the pattern says

1. **Four to five tiers, always the same shape.** A limited **Free** funnel, one or
   two **self-serve paid** tiers (the revenue workhorse), a **Business/Team** tier
   that unlocks collaboration and higher limits, and a **sales-led Enterprise**
   tier. Nobody sells a single flat plan.
2. **The self-serve entry price clusters at USD 5 to 29/mo.** Short.io $5, Bitly
   $10, Rebrandly $8-11, Dub $25, TinyURL ~$10-16. The first "real" paid tier is
   cheap on purpose: it converts hobbyists and removes the free ceiling.
3. **Annual gives roughly 15-20% off** and is the default toggle everywhere.
4. **Primary metering is links per month** (Bitly, Rebrandly) or **total active
   links** (Short.io, TinyURL), with **tracked clicks/events per month** as a
   second axis. Dub is the outlier that meters *events* rather than links, because
   its product is conversion attribution, not just shortening.
5. **These gates climb the ladder together**: custom domains (1 -> 3 -> 10 ->
   50 -> unlimited), seats (1 -> few -> unlimited), analytics retention (30 days ->
   1 year -> multi-year -> unlimited), and API rate limits.
6. **Enterprise is defined by capability, not volume**: SSO/SAML, audit logs,
   a contractual SLA (99.9% to 99.99%), dedicated support. This is the one place
   where feature-gating (not metering) is the lever, and it is remarkably
   consistent across all five vendors.

## 2. Recommended gating dimensions for quark

quark already ships almost every feature these vendors gate on, which is unusual
for a pre-revenue product. The job is to pick which existing capabilities become
plan boundaries. Recommended primary and secondary axes, each mapped to a real
feature:

**Primary metering axes (drive the tier ladder):**

| Axis | Why | Maps to |
|---|---|---|
| Links created / month | The universal shorthand for "how much product". Easy to explain, easy to meter, aligns cost with value. | `POST /` create path; `Record` per link |
| Tracked clicks (or events) / month | The real cost driver: every click is a `ClickEvent` through the analytics worker and, on cloud, a ClickHouse write. Metering this protects margin. | `src/analytics/` worker + ClickHouse sink |
| Analytics retention window | Cheapest lever to differentiate tiers at near-zero eng cost; it is a query/TTL policy, not a feature build. Matches the whole market (30d -> 1y -> multi-year). | ClickHouse TTL per tenant; `GET /:code/stats` |

**Secondary feature/limit gates (differentiate Team and Enterprise):**

| Axis | Why | Maps to |
|---|---|---|
| Custom domains per tenant | Classic upsell; the market ladders 1 -> unlimited. quark has this on the roadmap (P3, in progress). | multi-tenancy P3 custom domains (roadmap) |
| Workspace members / roles | Collaboration is the Team-tier trigger everywhere. quark already has M:N memberships and 4 roles. | `Membership`, roles `Owner/Admin/Member/Viewer` (P2b) |
| API tokens + rate limit / scopes | Higher rate limits and more named tokens are a natural paid gate; quark already has scoped tokens with per-token quota. | `src/auth.rs`, `docs/API-TOKENS.md` |
| SSO / OIDC per tenant | The single most consistent Enterprise gate in the market. quark ships per-tenant OIDC and hosted Keycloak. | multi-tenancy P2d/P2e; `docs/OIDC-LOGIN.md` |
| Webhooks + notification channels | Dub/Bitly reserve webhooks for higher tiers. quark has signed webhooks + Slack/Discord/Telegram and a durable Postgres outbox. | `src/webhooks/`, `docs/WEBHOOKS.md` |
| Conversion forwarding (GA4/Meta CAPI) + pixels | Attribution is a premium feature (Dub's whole pitch). quark forwards server-side. | `src/pixel.rs`, `docs/CONVERSION-FORWARDING.md` |
| Google Sheets sync | A convenience integration worth reserving above Free. | `docs/SHEETS.md` |
| SLA + audit logs + dedicated support | The Enterprise definition, matching all five vendors. SLA is contractual; audit logs are new/roadmap. | new/roadmap for audit logs |

**Deliberately NOT gated (keep in every tier, including Free):** the redirect hot
path itself, QR codes, tags/folders, geo/device redirect rules, A/B variants,
device-aware deep links, password-protected links, max-visits/TTL expiry, the UTM
builder, broken-link monitoring, and CSV/JSON import. These are cheap to serve and
are exactly the features that make quark feel complete at the entry tier, which is
the wedge against Bitly's famously stingy Free plan (5 links/mo).

## 3. Proposed quark cloud tiers

Four tiers: a genuinely useful **Free**, two self-serve paid tiers
(**Starter**, **Pro**), and a sales-led **Business/Enterprise**. Four is enough:
the market's 5th tier (Dub Advanced, Bitly Premium) exists mostly to price-ladder
huge volumes, which quark can fold into "Enterprise = custom volume". Prices shown
in USD and BRL (approx 1 USD ~ 5.4 BRL, 2026); round the BRL for local psychology
(e.g. R$19, R$49). Annual = about 2 months free (~17% off), matching the market.

### Free — USD 0 / R$ 0
The funnel and the OSS on-ramp. Deliberately more generous than Bitly Free so
quark wins the "just let me use it" comparison.
- 50 links/month, 1 custom domain, 1 member (Owner only)
- 10k tracked clicks/month, 30-day analytics retention
- All core redirect features: QR, tags, folders, geo/device rules, A/B variants,
  deep links, password protection, TTL/max-visits, UTM builder, CSV import
- 1 API token, low rate limit (e.g. 60 req/min)
- No webhooks, no Sheets sync, no conversion forwarding, no SSO
- Community support

### Starter — USD 9 / R$ 49 per month (annual ~USD 90 / R$ 490)
The hobbyist/solo-creator tier. Priced between Short.io ($5) and Dub Pro ($25).
- 1,000 links/month, 3 custom domains, 2 members
- 100k tracked clicks/month, 1-year retention
- Everything in Free, plus: webhooks + Slack/Discord/Telegram channels,
  Google Sheets sync, GA4/Meta conversion forwarding + pixels
- 3 API tokens, higher rate limit (e.g. 600 req/min)
- Email support

### Pro — USD 29 / R$ 149 per month (annual ~USD 290 / R$ 1490)
The team workhorse, where quark expects most revenue. Comparable to Short.io Team
($48) and Rebrandly Growth ($69-99), but cheaper as a challenger.
- 10,000 links/month, 25 custom domains, 10 members with roles
- 1M tracked clicks/month, 3-year retention
- Everything in Starter, plus: full role-based access (Owner/Admin/Member/Viewer),
  broken-link health monitoring, per-token scopes + quota controls, priority email
- Higher API rate limits (e.g. 3,000 req/min)

### Enterprise — custom (contact sales)
The capability tier, defined like the whole market defines it.
- Unlimited links, domains, members; custom tracked-click volume and retention
- SSO / OIDC per tenant + hosted Keycloak (already shipped)
- 99.9%+ SLA, audit logs (new/roadmap), dedicated support, custom API limits
- Optional single-tenant / dedicated infra, invoicing, DPA

**Rationale recap:** Free is the wedge (generous core, metered volume). Starter
unlocks *integrations* (webhooks, Sheets, pixels) because those are the first thing
a serious solo user wants and they cost little to serve. Pro unlocks *collaboration
and scale* (seats, roles, domains, retention) which is where willingness-to-pay
jumps. Enterprise is *capability* (SSO, SLA, audit) not volume, matching every
competitor and matching what quark already built (per-tenant OIDC, Keycloak).

## 4. Risks and open decisions

1. **Meter clicks vs links.** Links/month is easy to explain but does not track
   cost; clicks/events track cost but scare high-traffic users. Recommendation:
   gate on **both** (links as the headline number, tracked clicks as the fair-use
   ceiling), which is exactly what Short.io and Rebrandly do. Decide the overage
   behavior: soft cap (keep redirecting, stop *recording* analytics past the cap,
   as Bitly Free does) vs hard cap vs metered overage billing. Soft cap is
   friendliest and protects the redirect promise.
2. **Analytics retention is the cheapest lever but has a real cost.** Per-tenant
   ClickHouse TTL is trivial to implement, but multi-year retention on high-traffic
   tenants is a genuine storage cost. Confirm the ClickHouse cost curve per tenant
   before promising "3-year" and "unlimited" retention (see the per-tenant
   analytics work, multi-tenancy P4a).
3. **ClickHouse / infra cost per tenant.** The cloud runs shared Postgres + Valkey
   + ClickHouse. A single noisy tenant on the shared analytics sink can degrade
   others. Open question: at what tier (or tenant size) do you move a tenant to
   isolated analytics, and is that an Enterprise-only "dedicated infra" SKU? This
   also interacts with the tracked-clicks cap above.
4. **How the Claude API cost enters (LUC-41).** If any cloud feature calls the
   Claude API (e.g. an AI assist / natural-language link or analytics feature -
   new/roadmap, not currently in quark), that is a **variable per-request COGS**
   unlike the mostly-fixed infra cost. Options: (a) reserve AI features for Pro/
   Enterprise and absorb the cost in the higher margin; (b) meter AI usage
   separately as credits/add-on; (c) keep AI out of the metered plans entirely
   until usage is understood. Recommendation: treat AI as a **usage-metered add-on
   or a Pro+ perk with a fair-use cap**, never bundled into Free, so a single user
   cannot run up unbounded Claude spend on a $0 plan. This is the main billing
   decision LUC-41 must resolve.
5. **Custom domains are still roadmap (P3).** The proposal gates domains per tier,
   but the feature is in progress. If it slips, Starter/Pro still stand on
   links + clicks + integrations; domains can be layered in when P3 ships.
6. **Currency and market.** BRL-first or USD-first pricing? A BRL-anchored ladder
   (R$19/R$49/R$149) reads cheaper locally and dodges FX friction for Brazilian
   customers, but USD is the international default. Consider dual pricing with
   Stripe's local presentment.
7. **Free-plan abuse.** A generous Free tier on a *shortener* is a spam/malware
   magnet. The existing SSRF guard and per-IP rate limit help, but cloud Free needs
   its own abuse budget (verified email, lower rate limits, link scanning - the
   last is new/roadmap). Factor this into the Free limits.
8. **Seat-based vs flat pricing.** The proposal bundles a seat count per tier
   (market norm). An alternative is per-seat add-ons above the included count
   (Dub charges per extra user on some tiers). Decide whether extra seats are a
   hard cap (upgrade tier) or a metered add-on.
