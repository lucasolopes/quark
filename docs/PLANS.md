**English** · [Português](PLANS.PT_BR.md)

# Plans and entitlement

quark Cloud gates a small number of features and quotas by plan. This is an
Enterprise concern: a self-hosted Community install never has a plan and
never hits a limit. If you are running the AGPL build for yourself, this page
does not apply to you; see the note below.

## The Community edition applies no limit at all

`src/api/entitlement.rs` is the seam every gated code path calls through.
When quark is built without `--features ee`, both functions behind that seam
(`require`, `require_quota`) are Community stubs that always return `Ok`.
There is no plan lookup, no store round trip, and no way to configure a
limit into the Community build. This is a design invariant, not a default
that happens to be generous today: limiting a self-hosted install would
contradict the open-core split described in
[`specs/2026-08-03-luc19-open-core-design.md`](specs/2026-08-03-luc19-open-core-design.md).

Everything below describes the Enterprise build (`--features ee`), which is
what quark Cloud runs.

## The plan grid

Five plans, `Free` through `Custom`, live in `crate::ee::plan::Plan`. The
numbers are code, not configuration: changing a limit is a deploy, and it
applies to every tenant on that plan at once.

| | Free | Starter | Pro | Business | Custom |
|---|---|---|---|---|---|
| Domains | 3 | 10 | 50 | unlimited | unlimited¹ |
| Members | 1 | 3 | 10 | unlimited | unlimited¹ |
| Automation runs / month | 100 | 5,000 | 50,000 | 500,000 | unlimited¹ |
| Tracked clicks / month | 50,000 | 250,000 | 1,000,000 | 5,000,000 | unlimited¹ |
| Analytics retention | 30 days | 365 days | 730 days | 1,095 days | unlimited¹ |

¹ `Custom` is the negotiated tier for a contracted customer. Absent a
per-tenant override it is unlimited across the board. A per-tenant override
column (`plan_limits`) is designed but not built yet; it has no consumer
until a customer actually needs a narrower Custom limit than "everything".

The member ceiling is enforced when a caller redeems an invite through
`POST /admin/invites/:token/accept` (model A, no IdP provisioned for the
tenant). For a tenant with its own IdP provisioned (Keycloak/model B),
membership is instead granted at first login off the group claim, and that
path does not apply the member quota yet. A tenant on that model can exceed
its member ceiling by having enough distinct users log in. This is a known
gap, accepted for this phase, tracked as LUC-148; closing it needs the
billing context phase 2 brings.

### Features (binary, not a ceiling)

| | Free | Starter | Pro | Business | Custom |
|---|---|---|---|---|---|
| Webhooks | – | ✓ | ✓ | ✓ | ✓ |
| Integrations (Sheets, pixels) | – | ✓ | ✓ | ✓ | ✓ |
| Broken-link monitoring* | – | – | ✓ | ✓ | ✓ |
| Scoped API tokens* | – | – | ✓ | ✓ | ✓ |
| SSO | – | – | – | ✓ | ✓ |

\* Not enforced in this phase. `Feature` (`src/api/entitlement.rs`) has no
`HealthMonitoring` or `TokenScopes` variant yet, no handler checks either one,
and `GET /admin/plan` cannot list them as unlocked because they do not exist
in code. These two rows describe the commercial roadmap, not current
behavior; they land in code together with the slice of work that wires them
to a real handler.

Slack (`src/api/slack.rs`) has no plan gate at all and is not part of the
commercial grid in this phase; connecting it is free on every plan, Free
included.

The monthly counters (automation runs, tracked clicks) are designed but not
enforced yet; they share a monthly counting mechanism that phase 3 builds.
Today only the row-count quotas (domains, members) and the Webhooks,
Integrations, and SSO features are actually gated. Broken-link monitoring and
scoped API tokens are not, per the note above.

## What a denial looks like

A feature or quota a plan does not unlock answers `402 Payment Required`,
never `403`: the caller is authorized, what is missing is plan. The body
names what was hit and where to go:

```json
{
  "error": "plan_limit_reached",
  "limit": "webhooks",
  "allowed": null,
  "upgrade_to": "starter"
}
```

`allowed` is the ceiling that was hit for a quota (e.g. `3` for domains), or
`null` for a binary feature. `upgrade_to` is the cheapest plan that lifts the
limit, computed from the same grid above so the panel never has to guess or
carry its own copy.

## What the redirect path never does

`Plan` and the entitlement seam are never consulted on the hot redirect path
(`src/api/links.rs`, `src/domain_router.rs`, `src/cache/mod.rs`). A plan
check there would add a store or cache round trip to the single
highest-traffic request in the system for a decision that only matters on
writes. Enforcement happens on the admin/write side: creating a domain,
creating a webhook, inviting a member, and so on.

## Reading the current plan

`GET /admin/plan` returns the caller's tenant's plan, its ceilings, and the
features it unlocks:

```json
{
  "plan": "starter",
  "limits": {
    "domains": 10,
    "members": 3,
    "automation_per_month": 5000,
    "tracked_clicks_per_month": 250000,
    "retention_days": 365
  },
  "features": ["webhooks", "integrations"]
}
```

A `null` limit means unlimited. The admin panel renders its plan/usage view
from this endpoint rather than carrying its own copy of the grid, which
would drift the first time a limit changes.

Any credential that passes `admin_guard` with `Scope::LinksRead` or higher
can read this. It is not sensitive, and every tenant already knows its own
plan from the product it experiences.

## Changing a tenant's plan

There is no payment gateway yet (that is phase 2). Until then, changing a
tenant's plan is an operator action:

```
PUT /admin/tenants/{id}/plan
x-admin-token: <QUARK_ADMIN_TOKEN>
Content-Type: application/json

{ "plan": "starter" }
```

Two things make this endpoint different from every other admin route:

- **It requires the break-glass `QUARK_ADMIN_TOKEN` directly**, compared in
  constant time, and nothing else: not a tenant API token, not a session,
  not any credential `admin_guard` would resolve on a tenant's behalf. This
  is deliberate: `Plan::Custom` grants everything unlimited, and `"custom"`
  is a string the parser recognizes like any other plan name. If a tenant
  credential could write this column, any customer could promote itself to
  unlimited. Only the operator holding the deploy's admin token can change a
  plan.
- **An unrecognized plan string is rejected with `400`**, not silently
  accepted. `Plan::from_stored` (used on every read) falls back to `Free` on
  an unknown string, which is the safe choice for a read: a corrupt value in
  the store must not take the product down or grant more than intended. On
  a write, that same fallback would be dangerous in the other direction: a
  typo like `"starterr"` would silently downgrade the tenant to Free instead
  of failing loudly. The write handler compares the parsed plan's canonical
  string back against what was sent and rejects anything that does not
  round-trip.

The change takes effect immediately on the node that handled this request:
the handler calls `st.ee.plans.invalidate(tenant)` after writing the plan, so
that node's very next request does not wait out the plan cache's 60-second
TTL. `PlanCache` is per-process, with no cross-node invalidation, so on a
multi-replica deploy the other nodes keep answering from their own cache
until it converges on its own, within the same 60-second TTL. Wiring
cross-node invalidation (the pub/sub channel `src/invalidate.rs` already uses
for cache entries) is future work.

## The pricing page is copy, not source

The plan grid your marketing site or pricing page shows is copy, kept in
sync by hand. `crate::ee::plan::Plan` in this repository is the only source
of truth for what a plan actually enforces. If the two ever disagree, the
code wins, and the pricing page needs to be fixed.
