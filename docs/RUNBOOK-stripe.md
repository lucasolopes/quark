# Stripe billing: production runbook

This covers what an operator needs to stand up Stripe for quark Cloud: the
products and prices with their lookup keys, the Customer Portal
configuration, the webhook endpoint, dunning settings, and how to test the
whole flow locally and in Stripe's sandbox before going live. For what the
code does with all of this, see [`BILLING.md`](BILLING.md); for the plan
grid itself, see [`PLANS.md`](PLANS.md).

quark never stores a Stripe price id in code or in an env var. Every price
lookup goes through a `lookup_key`, so the values below are a contract, not
a suggestion: get the key names wrong in the dashboard and checkout fails
with a `503` (`no active stripe price for lookup key` in the logs), even
though nothing in the deploy changed.

## 1. Create the six products and prices

Six self-service plan/cycle pairs, each with a stable lookup key
(`src/ee/stripe/map.rs` is the source of truth for these names):

| Lookup key | Plan | Cycle |
|---|---|---|
| `starter-monthly` | Starter | Monthly |
| `starter-yearly` | Starter | Yearly |
| `pro-monthly` | Pro | Monthly |
| `pro-yearly` | Pro | Yearly |
| `business-monthly` | Business | Monthly |
| `business-yearly` | Business | Yearly |

For each of the three plans (Starter, Pro, Business):

1. Create one Stripe **product** (e.g. "quark Starter").
2. Add a **monthly price** on it, multi-currency with both USD and BRL
   amounts on the same price object (Stripe's multi-currency prices, not two
   separate prices). Set its lookup key to the `-monthly` value above.
3. Add a **yearly price** the same way, with the `-yearly` lookup key.

Multi-currency on one price is what lets quark pass `currency: "usd"` or
`currency: "brl"` on the Checkout Session without needing a second price id
per currency. Prices for Free and Custom are not created: Free has nothing
to buy, Custom is negotiated through the operator escape hatch
(`PUT /admin/tenants/{id}/plan`, documented in `PLANS.md`) and never goes
through Checkout.

Amounts are not repeated here; they are marketing copy kept in sync by hand,
sourced from [`DECISAO-planos-e-pricing-cloud.md`](DECISAO-planos-e-pricing-cloud.md).

## 2. Configure the Customer Portal

In the Stripe Dashboard, under Settings → Billing → Customer Portal:

- **Products customers can switch to**: restrict to exactly the three
  products created above (Starter, Pro, Business). Do not leave "all
  products" selected; a customer should never be able to portal their way
  onto a product quark's plan map doesn't recognize.
- **Quantities**: turn off "customers can update quantities". quark's
  subscriptions are always quantity 1; letting a customer change it would
  desync billing from the plan the workspace actually has.
- **Cancellation**: allow cancellation, and set it to take effect **at the
  end of the current billing period**, not immediately. This matches the
  dunning table in `BILLING.md`: the workspace keeps its plan through
  `active`/`trialing`/`past_due` and only drops to Free once the
  subscription actually reaches a terminal status.
- **Downgrade timing**: same rule, at period end, not prorated immediately.

Save and publish the portal configuration; a portal session created against
an unpublished configuration fails.

## 3. Create the webhook endpoint

In the Dashboard, under Developers → Webhooks, add an endpoint:

- **URL**: `https://<backend>/stripe/webhook` (the backend's public
  hostname, not the panel's `QUARK_STRIPE_PANEL_URL`).
- **Events to send**, exactly these six:
  - `checkout.session.completed`
  - `customer.subscription.created`
  - `customer.subscription.updated`
  - `customer.subscription.deleted`
  - `invoice.paid`
  - `invoice.payment_failed`

quark acknowledges every other event type with a `200` and a log line but
does not act on it; there's no need to subscribe to more than these six.

After creating the endpoint, reveal its signing secret (`whsec_...`) and set
it as `QUARK_STRIPE_WEBHOOK_SECRET`. Set `QUARK_STRIPE_SECRET_KEY` to the
account's secret key (`sk_live_...` in production, `sk_test_...` in the
sandbox) and `QUARK_STRIPE_PANEL_URL` to the panel's base URL (no trailing
slash needed; quark normalizes it). All three env vars must be set together;
see `BILLING.md` for what happens if one is missing.

## 4. Smart Retries and dunning emails

Under Settings → Billing → Revenue recovery:

- Turn on **Smart Retries** so Stripe schedules retry attempts for a failed
  invoice payment instead of failing it once and giving up. This is what
  makes the `past_due` window in the dunning table meaningful: without
  retries, a single declined card would go straight to `unpaid`/canceled.
- Turn on Stripe's own **customer emails** for payment failure and upcoming
  renewal. quark does not send its own dunning email; Stripe's automatic
  emails cover the launch. A quark-branded dunning email is future work, not
  required for this phase.

## 5. Local testing with the Stripe CLI

Forward events to a local backend instead of registering a public webhook
endpoint for dev:

```bash
stripe listen --forward-to localhost:8080/stripe/webhook
```

The CLI prints a `whsec_...` for the forwarding session; use that as
`QUARK_STRIPE_WEBHOOK_SECRET` while testing locally (it's different from the
dashboard endpoint's secret). Trigger individual events with `stripe
trigger`, e.g. `stripe trigger checkout.session.completed`, to exercise the
webhook handler without going through a real Checkout session.

## 6. Test clocks: renewal and dunning in the sandbox

Stripe's test clocks (Dashboard → Developers → Test clocks, sandbox only)
advance a customer's simulated time so you can watch a subscription actually
renew or fail without waiting a real billing cycle:

1. Create a test clock, attach a test customer to it, and run through
   Checkout for that customer (test mode, test card `4242 4242 4242 4242`).
2. Advance the clock past the 14-day trial end. Confirm the workspace stays
   on the paid plan (`trialing` → `active`) and the trial-once marker (the
   workspace's stored Stripe subscription id) is set, so a second checkout
   for the same workspace does not grant a trial again.
3. Advance the clock to the next renewal with a card known to fail (Stripe's
   documented test cards include ones that always decline). Confirm the
   subscription goes `past_due`, the workspace keeps its plan, Smart Retries
   fires, and after retries are exhausted the subscription reaches a
   terminal status and the workspace drops to Free.
4. Confirm each of the six webhook events arrived exactly once by checking
   `stripe_events` (the idempotency ledger); a redelivered event should be
   acknowledged `200` without a second plan write.

## 7. Invoicing and tax

Stripe Tax is not enabled for this launch. For a Brazilian merchant, sales
tax (ISS, via NFS-e) is not something Stripe calculates or files; it is a
process that happens outside Stripe, against the same charge records. Do not
turn on Stripe Tax expecting it to produce a valid Brazilian nota fiscal; it
doesn't, and there is no code path here that depends on it being on or off.
