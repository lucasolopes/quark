**English** · [Português](BILLING.PT_BR.md)

# Billing

quark Cloud charges through Stripe. This page covers what an Owner sees and
does; the operator side (creating products, wiring the webhook) is
[`RUNBOOK-stripe.md`](RUNBOOK-stripe.md). For the plan grid itself (what each
plan unlocks, how quotas are enforced), see [`PLANS.md`](PLANS.md). For
prices, see [`DECISAO-planos-e-pricing-cloud.md`](DECISAO-planos-e-pricing-cloud.md);
this page does not repeat numbers that live there.

Community has no billing at all. There is no Stripe code in the AGPL core,
no env var it reacts to, and nothing to configure. Everything below is
Enterprise (`--features ee`), and only runs when Stripe is configured.

## Turning it on

Three environment variables, all or nothing:

- `QUARK_STRIPE_SECRET_KEY`
- `QUARK_STRIPE_WEBHOOK_SECRET`
- `QUARK_STRIPE_PANEL_URL`

If any one of the three is missing or empty, billing stays off: the checkout,
portal, and webhook endpoints all answer `404`, exactly as if the routes did
not exist. A self-hosted Enterprise build without Stripe keeps working in
full; plan limits (phase 1) are enforced independently of the payment
gateway. There is no partial state where billing is half-configured.

## How an Owner subscribes

Subscribing is a Stripe-hosted flow, not a form quark renders itself. The
Owner (only the Owner; see below) picks a plan and billing cycle in the
panel, and quark asks Stripe for a Checkout Session URL and redirects there.
Card entry, 3-D Secure, and any local payment method Stripe offers all
happen on Stripe's page, never on quark's.

A few decisions are locked in at that first checkout:

- **Currency.** The Owner chooses USD or BRL on the first checkout. Stripe
  then locks that currency to the customer; every subsequent charge for that
  workspace uses the same one. Switching a customer's currency after the
  fact is a manual operator action outside Stripe (see "Not supported"
  below), not something the product does automatically.
- **Trial.** A workspace that has never had a subscription gets 14 days
  free, no card required, exactly once. The marker is not the trial having
  been used before; it is whether the workspace has ever had a subscription
  id recorded. Resubscribing after a cancellation does not grant a second
  trial.

Only the Owner role can start a checkout or open the Customer Portal.
Admins, Members, and Viewers get `403` from both endpoints; the check reads
the caller's session and role, not an API token scope, because billing is a
logged-in-browser operation, not something an automation token should be
able to trigger.

## What the Customer Portal handles

Once a workspace has a Stripe customer, the Owner can open Stripe's hosted
Customer Portal from the panel. That one surface covers upgrade, downgrade,
cancellation, updating the card on file, and downloading past invoices.
quark does not build its own equivalent of any of these; the portal is the
single place to change or leave a plan once you are already a paying
customer.

## Plan states

quark keeps a subscription's Stripe status and its price's lookup key in
sync with the workspace's plan on every relevant webhook. Not every Stripe
status keeps the plan the subscription paid for:

| Stripe status | Effect on the workspace's plan |
|---|---|
| `active` | Keeps the paid plan. |
| `trialing` | Keeps the paid plan (this is the free-trial window). |
| `past_due` | Keeps the paid plan, through Stripe's Smart Retries window. |
| `canceled` | Drops to Free. |
| `unpaid` | Drops to Free. |
| `incomplete_expired` | Drops to Free. |
| `paused` | Drops to Free. |

`past_due` deliberately does not downgrade immediately: a card that failed
once and gets retried successfully a few days later should not have
interrupted the customer's plan in between. The downgrade only happens once
Stripe gives up (or the subscription is explicitly canceled).

## Downgrades never delete anything

Dropping to Free, whether through an explicit downgrade in the portal or
through the dunning table above, never deletes a resource that is now over
the Free ceiling. A workspace with 8 domains that lands on Free (limit: 3)
keeps all 8; it just cannot create a ninth until it is back under the limit
or on a plan whose ceiling allows it. The plan layer only ever blocks new
creation, the same enforcement point phase 1 already uses for every other
quota. There is no background job that prunes a workspace down to its new
plan's ceiling.

The member ceiling follows this same rule, including for a workspace whose
members join through its own SSO provider (Keycloak/model B, LUC-148): a
downgrade never removes an existing member, and nobody who already has a
membership is ever logged out or blocked from signing back in because the
workspace is now over the new ceiling. What a downgrade DOES block is a
BRAND NEW member joining: the next person who was never a member before and
tries to log in through the group claim gets the login refused
(`member_limit_reached`) until the workspace is back under the ceiling or
on a plan whose ceiling allows it, the same shape as the "cannot create a
ninth domain" rule above.

SSO access itself is unaffected by a downgrade in a different sense: `Sso` is
a feature gate on creating or editing the tenant's IdP configuration, not on
using one that is already configured. A workspace that configured SSO on a
paid plan and later drops to a plan without the `Sso` feature keeps signing
members in through that IdP exactly as before; only setting up (or changing)
the configuration requires the feature again. The member ceiling above still
applies to every login on that IdP regardless of the `Sso` feature.

## Not supported yet

- **Currency change for an existing customer.** Stripe does not let you flip
  a customer's currency once it is set; changing it is a manual operator
  action (typically: cancel and resubscribe with the new currency), not a
  self-service flow.
- **Panel billing screen and the 402 upgrade prompt.** The panel does not
  yet render plan/usage or a call-to-action when a request comes back `402`.
  That lands with the billing landing page.
- **Custom domain for checkout.** Checkout and the portal use Stripe's own
  domain until custom-domain support (LUC-147) lands.
- **Soft caps, the monthly counters, and the automation ceiling.** Those are
  phase 3.
- **Stripe Tax.** Not enabled; see the runbook for how invoicing works for a
  Brazilian merchant without it.
