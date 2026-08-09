//! Billing endpoints (LUC-41 phase 2). Covered by `src/ee/LICENSE`.
//!
//! Checkout and portal are Owner-only, resolved from the session like
//! `admin_tenants_delete`: an API token carries scopes, not a role, so
//! billing is a logged-in-browser operation. Neither endpoint writes `plan`;
//! that is the webhook's job (spec D4).

use super::*;
use crate::ee::plan::Plan;
use crate::ee::stripe::map::{lookup_key, Cycle};

/// Resolves the session and requires the Owner role on its workspace.
/// 401 without a session, 403 with one that is not Owner, 503 on store error.
pub(super) async fn require_owner(
    st: &AppState,
    headers: &HeaderMap,
) -> Result<(u64, crate::tenant::TenantId), StatusCode> {
    let Some(session) = current_session(st, headers).await else {
        return Err(StatusCode::UNAUTHORIZED);
    };
    match st
        .store
        .get_membership(session.user_id, session.tenant_id)
        .await
    {
        Ok(Some(m)) if m.role == crate::tenant::Role::Owner => {
            Ok((session.user_id, session.tenant_id))
        }
        Ok(Some(_)) => Err(StatusCode::FORBIDDEN),
        Ok(None) => Err(StatusCode::UNAUTHORIZED),
        Err(_) => Err(StatusCode::SERVICE_UNAVAILABLE),
    }
}

#[derive(serde::Deserialize)]
pub(crate) struct CheckoutReq {
    pub plan: String,
    pub cycle: String,
    pub currency: String,
}

/// `POST /admin/billing/checkout`: creates (or reuses) the tenant's Stripe
/// customer and answers the hosted Checkout URL.
pub(crate) async fn admin_billing_checkout(
    State(st): State<Arc<AppState>>,
    headers: HeaderMap,
    Json(req): Json<CheckoutReq>,
) -> Response {
    if !st.multi_tenant {
        return StatusCode::NOT_FOUND.into_response();
    }
    let Some(billing) = st.ee.billing.clone() else {
        return StatusCode::NOT_FOUND.into_response();
    };
    let (_user, tenant) = match require_owner(&st, &headers).await {
        Ok(v) => v,
        Err(status) => return status.into_response(),
    };

    // Validate the request before any Stripe call.
    let plan = Plan::from_stored(&req.plan);
    if plan.as_str() != req.plan {
        return (StatusCode::BAD_REQUEST, "unknown plan").into_response();
    }
    let Some(cycle) = Cycle::parse(&req.cycle) else {
        return (StatusCode::BAD_REQUEST, "cycle must be monthly or yearly").into_response();
    };
    let Some(key) = lookup_key(plan, cycle) else {
        return (StatusCode::BAD_REQUEST, "plan is not self-service").into_response();
    };
    // Spec D5: the currency is a first-checkout decision and Stripe locks it
    // on the customer afterwards, so it is explicit here, never IP-guessed.
    let currency = match req.currency.as_str() {
        "usd" => stripe_types::Currency::USD,
        "brl" => stripe_types::Currency::BRL,
        _ => return (StatusCode::BAD_REQUEST, "currency must be usd or brl").into_response(),
    };

    // Customer: reuse or create-and-persist.
    let customer_id = match st.store.get_stripe_customer_id(tenant).await {
        Ok(Some(id)) => id,
        Ok(None) => {
            let tenant_row = match st.store.get_tenant(tenant).await {
                Ok(Some(t)) => t,
                Ok(None) => return StatusCode::NOT_FOUND.into_response(),
                Err(_) => return StatusCode::SERVICE_UNAVAILABLE.into_response(),
            };
            let created = stripe_core::customer::CreateCustomer::new()
                .name(tenant_row.name.as_str())
                .metadata([(String::from("tenant_id"), tenant.0.to_string())])
                .send(&billing.client)
                .await;
            let customer = match created {
                Ok(c) => c,
                Err(e) => {
                    tracing::warn!(error = %e, tenant_id = tenant.0, "stripe customer create failed");
                    return StatusCode::SERVICE_UNAVAILABLE.into_response();
                }
            };
            if st
                .store
                .set_stripe_customer_id(tenant, customer.id.as_str())
                .await
                .is_err()
            {
                return StatusCode::SERVICE_UNAVAILABLE.into_response();
            }
            // The write above is an idempotent claim (first checkout wins),
            // not a blind overwrite: a concurrent request for the same
            // tenant may have created and persisted a different customer
            // first. Re-read to learn who actually won, so both requests
            // converge on the same Stripe customer instead of splitting
            // into two. The loser's `customer` is orphaned in Stripe
            // (harmless: it never gets a subscription).
            match st.store.get_stripe_customer_id(tenant).await {
                Ok(Some(won)) => {
                    if won != customer.id.as_str() {
                        tracing::info!(
                            tenant_id = tenant.0,
                            created = customer.id.as_str(),
                            winner = won.as_str(),
                            "stripe customer race: concurrent checkout won, discarding this create"
                        );
                    }
                    won
                }
                Ok(None) => {
                    // Can't happen: this branch just claimed the column (or
                    // lost the race to another claim), so it is never NULL
                    // here. Fall back to the locally created id defensively.
                    customer.id.to_string()
                }
                Err(_) => return StatusCode::SERVICE_UNAVAILABLE.into_response(),
            }
        }
        Err(_) => return StatusCode::SERVICE_UNAVAILABLE.into_response(),
    };

    // Resolve the price by lookup key (spec D3: no price IDs in code or env).
    let prices = match stripe_product::price::ListPrice::new()
        .lookup_keys(vec![key.to_string()])
        .active(true)
        .send(&billing.client)
        .await
    {
        Ok(p) => p,
        Err(e) => {
            tracing::warn!(error = %e, "stripe price list failed");
            return StatusCode::SERVICE_UNAVAILABLE.into_response();
        }
    };
    let Some(price) = prices.data.first() else {
        // The dashboard is missing a price for this key: an operator problem,
        // not a caller problem. The runbook documents the keys.
        tracing::error!(lookup_key = key, "no active stripe price for lookup key");
        return StatusCode::SERVICE_UNAVAILABLE.into_response();
    };

    // Trial once per tenant (spec D6): a tenant that ever had a subscription
    // does not get another trial by resubscribing.
    let existing_subscription_id = match st.store.get_stripe_subscription_id(tenant).await {
        Ok(v) => v,
        Err(_) => return StatusCode::SERVICE_UNAVAILABLE.into_response(),
    };
    let had_subscription = existing_subscription_id.is_some();

    // A recorded subscription id is not proof the tenant is currently
    // unsubscribed: it may still be active, trialing, or past_due, in which
    // case a second checkout would open a second paid subscription for the
    // same tenant (double billing). Re-fetch it live, the same way the
    // webhook does, instead of trusting whatever status is cached locally.
    if let Some(sub_id) = existing_subscription_id.as_deref() {
        let fetched = stripe_billing::subscription::RetrieveSubscription::new(
            stripe_shared::SubscriptionId::from(sub_id),
        )
        .send(&billing.client)
        .await;
        let current = match fetched {
            Ok(s) => s,
            Err(e) => {
                tracing::warn!(error = %e, tenant_id = tenant.0, "stripe subscription re-fetch failed");
                return StatusCode::SERVICE_UNAVAILABLE.into_response();
            }
        };
        let paid = current
            .items
            .data
            .first()
            .and_then(|item| item.price.lookup_key.as_deref())
            .and_then(crate::ee::stripe::map::plan_for_lookup_key)
            .unwrap_or(Plan::Free);
        if crate::ee::stripe::map::effective_plan(&current.status, paid) != Plan::Free {
            return (
                StatusCode::CONFLICT,
                Json(serde_json::json!({
                    "error": "subscription_active",
                    "manage_url": null
                })),
            )
                .into_response();
        }
    }

    use stripe_checkout::checkout_session::{
        CreateCheckoutSession, CreateCheckoutSessionLineItems,
        CreateCheckoutSessionSubscriptionData, CreateCheckoutSessionSubscriptionDataTrialSettings,
        CreateCheckoutSessionSubscriptionDataTrialSettingsEndBehavior,
        CreateCheckoutSessionSubscriptionDataTrialSettingsEndBehaviorMissingPaymentMethod,
    };
    let mut sub_data = CreateCheckoutSessionSubscriptionData {
        metadata: Some([(String::from("tenant_id"), tenant.0.to_string())].into()),
        ..Default::default()
    };
    if !had_subscription {
        sub_data.trial_period_days = Some(14);
        sub_data.trial_settings = Some(CreateCheckoutSessionSubscriptionDataTrialSettings::new(
            CreateCheckoutSessionSubscriptionDataTrialSettingsEndBehavior::new(
                CreateCheckoutSessionSubscriptionDataTrialSettingsEndBehaviorMissingPaymentMethod::Cancel,
            ),
        ));
    }
    let session = match CreateCheckoutSession::new()
        .mode(stripe_checkout::CheckoutSessionMode::Subscription)
        .customer(customer_id.as_str())
        .client_reference_id(tenant.0.to_string())
        .currency(currency)
        .line_items(vec![CreateCheckoutSessionLineItems {
            price: Some(price.id.to_string()),
            quantity: Some(1),
            ..Default::default()
        }])
        .subscription_data(sub_data)
        .success_url(format!(
            "{}/settings/billing?checkout=success",
            billing.panel_url
        ))
        .cancel_url(format!(
            "{}/settings/billing?checkout=cancel",
            billing.panel_url
        ))
        .send(&billing.client)
        .await
    {
        Ok(s) => s,
        Err(e) => {
            tracing::warn!(error = %e, tenant_id = tenant.0, "stripe checkout session failed");
            return StatusCode::SERVICE_UNAVAILABLE.into_response();
        }
    };
    match session.url {
        Some(url) => Json(serde_json::json!({ "url": url })).into_response(),
        None => StatusCode::SERVICE_UNAVAILABLE.into_response(),
    }
}

/// `POST /admin/billing/portal`: hosted Customer Portal session. 404 while
/// the tenant has no Stripe customer (nothing to manage yet).
pub(crate) async fn admin_billing_portal(
    State(st): State<Arc<AppState>>,
    headers: HeaderMap,
) -> Response {
    if !st.multi_tenant {
        return StatusCode::NOT_FOUND.into_response();
    }
    let Some(billing) = st.ee.billing.clone() else {
        return StatusCode::NOT_FOUND.into_response();
    };
    let (_user, tenant) = match require_owner(&st, &headers).await {
        Ok(v) => v,
        Err(status) => return status.into_response(),
    };
    let customer_id = match st.store.get_stripe_customer_id(tenant).await {
        Ok(Some(id)) => id,
        Ok(None) => return StatusCode::NOT_FOUND.into_response(),
        Err(_) => return StatusCode::SERVICE_UNAVAILABLE.into_response(),
    };
    let portal = match stripe_billing::billing_portal_session::CreateBillingPortalSession::new()
        .customer(customer_id.as_str())
        .return_url(format!("{}/settings/billing", billing.panel_url))
        .send(&billing.client)
        .await
    {
        Ok(p) => p,
        Err(e) => {
            tracing::warn!(error = %e, tenant_id = tenant.0, "stripe portal session failed");
            return StatusCode::SERVICE_UNAVAILABLE.into_response();
        }
    };
    Json(serde_json::json!({ "url": portal.url })).into_response()
}

/// `GET /admin/billing/catalog`: the whole plan grid for the panel. Read
/// scope on purpose: any member may LOOK at the grid (spec D3); the actions
/// stay Owner-only. Limits and features come from the code catalog (phase 1
/// D6: the panel never carries its own copy); prices come from Stripe
/// through the 12h cache (spec D2).
pub(crate) async fn admin_billing_catalog(
    State(st): State<Arc<AppState>>,
    headers: HeaderMap,
) -> Response {
    if !st.multi_tenant {
        return StatusCode::NOT_FOUND.into_response();
    }
    let p = match admin_guard(&st, &headers, Scope::LinksRead).await {
        Ok(p) => p,
        Err(status) => return status.into_response(),
    };
    let current = crate::ee::api::entitlement::plan_of(&st, p.tenant).await;
    let (prices, currency_locked) = match &st.ee.billing {
        Some(b) => (
            b.catalog_prices().await,
            locked_currency(&st, b, p.tenant).await,
        ),
        None => (None, None),
    };
    let plans: Vec<serde_json::Value> = Plan::ALL
        .into_iter()
        .map(|plan| {
            let l = plan.limits();
            let features: Vec<&'static str> = crate::api::entitlement::Feature::ALL
                .into_iter()
                .filter(|f| plan.allows(*f))
                .map(crate::api::entitlement::Feature::as_str)
                .collect();
            let plan_prices = prices.as_ref().and_then(|cp| {
                let m = lookup_key(plan, Cycle::Monthly)?;
                let y = lookup_key(plan, Cycle::Yearly)?;
                Some(serde_json::json!({
                    "monthly": cp.by_lookup_key.get(m),
                    "yearly": cp.by_lookup_key.get(y),
                }))
            });
            serde_json::json!({
                "plan": plan.as_str(),
                "limits": {
                    "domains": l.domains,
                    "members": l.members,
                    "automation_per_month": l.automation_per_month,
                    "tracked_clicks_per_month": l.tracked_clicks_per_month,
                    "retention_days": l.retention_days,
                },
                "features": features,
                "prices": plan_prices,
            })
        })
        .collect();
    Json(serde_json::json!({
        "current_plan": current.as_str(),
        "currency_locked": currency_locked,
        "prices_available": prices.is_some(),
        "plans": plans,
    }))
    .into_response()
}

/// The tenant's Stripe-locked currency, for the catalog's currency-picker
/// display. Cached alongside the catalog prices (`StripeBilling::currency_cache`),
/// same 12h/stale-serves-old pattern as `catalog_prices`. `None` (no customer
/// yet, or any lookup failure) is the safe fallback: it just means the free
/// checkout currency picker stays open, and Stripe itself would reject a
/// currency mismatched with an existing subscription at checkout time anyway.
async fn locked_currency(
    st: &AppState,
    billing: &crate::ee::stripe::StripeBilling,
    tenant: crate::tenant::TenantId,
) -> Option<String> {
    {
        let cache = billing.currency_cache.read().await;
        if let Some((fetched_at, value)) = cache.get(&tenant) {
            if fetched_at.elapsed() < crate::ee::stripe::CATALOG_TTL {
                return value.clone();
            }
        }
    }
    let customer_id = match st.store.get_stripe_customer_id(tenant).await {
        Ok(Some(id)) => id,
        Ok(None) => return None,
        Err(_) => return None,
    };
    let fetched = stripe_core::customer::RetrieveCustomer::new(customer_id.as_str())
        .send(&billing.client)
        .await;
    let currency = match fetched {
        Ok(stripe_core::customer::RetrieveCustomerReturned::Customer(c)) => {
            c.currency.map(|c| c.to_string())
        }
        // A deleted customer has no currency to lock against; treat like "no
        // customer yet" rather than surfacing an error for something the
        // caller can't act on.
        Ok(stripe_core::customer::RetrieveCustomerReturned::DeletedCustomer(_)) => None,
        Err(e) => {
            tracing::warn!(error = %e, tenant_id = tenant.0, "stripe customer retrieve failed");
            // Stale-serves-old, same as `catalog_prices`: fall back to
            // whatever was cached (possibly still nothing) instead of
            // treating a transient Stripe failure as "definitely no lock".
            return billing
                .currency_cache
                .read()
                .await
                .get(&tenant)
                .and_then(|(_, v)| v.clone());
        }
    };
    billing
        .currency_cache
        .write()
        .await
        .insert(tenant, (std::time::Instant::now(), currency.clone()));
    currency
}

/// Applies a subscription's current state to its tenant: status plus the
/// price lookup key decide the effective plan (spec D8), written through the
/// phase 1 seam and cache. Public to the crate's tests; the only production
/// caller is the webhook below.
pub async fn apply_subscription(
    st: &AppState,
    sub: &stripe_shared::Subscription,
) -> Result<(), StatusCode> {
    // The webhook arrives with a customer id; metadata's tenant_id is the
    // fallback for events older than the reverse index.
    let customer_id = match &sub.customer {
        stripe_types::Expandable::Id(id) => id.to_string(),
        stripe_types::Expandable::Object(c) => c.id.to_string(),
    };
    let tenant = match st.store.find_tenant_by_stripe_customer(&customer_id).await {
        Ok(Some(t)) => t,
        Ok(None) => {
            match sub
                .metadata
                .get("tenant_id")
                .and_then(|v| v.parse::<u64>().ok())
            {
                Some(id) => crate::tenant::TenantId(id),
                None => {
                    // Orphan event (an old environment, a deleted tenant):
                    // acknowledge, never leave it retrying forever.
                    tracing::warn!(customer = %customer_id, "stripe event for unknown tenant");
                    return Ok(());
                }
            }
        }
        Err(_) => return Err(StatusCode::SERVICE_UNAVAILABLE),
    };
    let paid = sub
        .items
        .data
        .first()
        .and_then(|item| item.price.lookup_key.as_deref())
        .and_then(crate::ee::stripe::map::plan_for_lookup_key)
        .unwrap_or(Plan::Free);
    let effective = crate::ee::stripe::map::effective_plan(&sub.status, paid);

    // Out-of-order guard for the cancel-and-resubscribe cycle: an owner who
    // cancels subscription A and immediately creates B can have Stripe
    // deliver `created`(B, paid) before `deleted`(A, canceled). The D4
    // re-fetch above only protects against reordering WITHIN one
    // subscription's own events; it does nothing for two different
    // subscriptions on the same customer racing each other. Without this
    // guard, `deleted`(A) would land last, re-resolve to Free (A really is
    // canceled), and clobber the tenant with the terminal state of a
    // subscription that is no longer the one paying, downgrading an actual
    // paying tenant. So: a terminal (Free) result only gets written when
    // `sub` is still the tenant's CURRENT subscription id. A terminal event
    // for a subscription that has already been superseded is a stale event
    // about a stale subscription; ack it and leave the tenant's real
    // (newer) plan alone. Non-terminal (paid) results always apply: the
    // newest paid subscription wins regardless of arrival order.
    if effective == Plan::Free {
        let current = st.store.get_stripe_subscription_id(tenant).await;
        match current {
            Ok(Some(ref current_id)) if current_id != sub.id.as_str() => {
                tracing::info!(
                    tenant_id = tenant.0,
                    stale_subscription = sub.id.as_str(),
                    current_subscription = current_id.as_str(),
                    "stale terminal event for a superseded subscription, ignoring"
                );
                return Ok(());
            }
            Ok(_) => {}
            Err(_) => return Err(StatusCode::SERVICE_UNAVAILABLE),
        }
    }

    if st
        .store
        .set_tenant_plan(tenant, effective.as_str())
        .await
        .is_err()
    {
        return Err(StatusCode::SERVICE_UNAVAILABLE);
    }
    if st
        .store
        .set_stripe_subscription_id(tenant, sub.id.as_str())
        .await
        .is_err()
    {
        return Err(StatusCode::SERVICE_UNAVAILABLE);
    }
    st.ee.plans.invalidate(tenant).await;
    // Best-effort cross-node invalidation: other replicas' `PlanCache`
    // entries would otherwise only converge within `PLAN_TTL_SECS`.
    // `publish` is itself fail-open (logs and swallows), so no error
    // handling is needed here.
    if let Some(inv) = st.cache.invalidator() {
        inv.publish(&format!("plan:{}", tenant.0)).await;
    }
    tracing::info!(
        tenant_id = tenant.0,
        plan = effective.as_str(),
        status = ?sub.status,
        "plan updated from stripe subscription"
    );
    Ok(())
}

/// `POST /stripe/webhook`. Public route: the authentication IS the
/// `Stripe-Signature` header. 400 on a bad signature, 200 on a duplicate,
/// 5xx on our own failure so Stripe retries.
pub(crate) async fn stripe_webhook(
    State(st): State<Arc<AppState>>,
    headers: HeaderMap,
    body: String,
) -> Response {
    let Some(billing) = st.ee.billing.clone() else {
        return StatusCode::NOT_FOUND.into_response();
    };
    let Some(sig) = headers
        .get("stripe-signature")
        .and_then(|v| v.to_str().ok())
    else {
        return StatusCode::BAD_REQUEST.into_response();
    };
    let event = match stripe_webhook::Webhook::construct_event(&body, sig, &billing.webhook_secret)
    {
        Ok(e) => e,
        Err(e) => {
            tracing::warn!(error = %e, "stripe webhook signature rejected");
            return StatusCode::BAD_REQUEST.into_response();
        }
    };

    // Idempotency ledger (spec D4): one row per event id, replays are acked
    // and skipped. The row is RELEASED again on every 5xx below: otherwise
    // Stripe's retry of a failed delivery would hit the dedup and the change
    // would be lost for good.
    let event_id = event.id.to_string();
    match st
        .store
        .record_stripe_event(&event_id, event.type_.as_str(), now())
        .await
    {
        Ok(true) => {}
        Ok(false) => return StatusCode::OK.into_response(),
        Err(_) => return StatusCode::SERVICE_UNAVAILABLE.into_response(),
    }
    // Best-effort ledger release before answering 5xx (see above).
    let fail = |st: Arc<AppState>, event_id: String, status: StatusCode| async move {
        let _ = st.store.delete_stripe_event(&event_id).await;
        status.into_response()
    };

    use stripe_webhook::EventObject;
    match event.data.object {
        EventObject::CheckoutSessionCompleted(session) => {
            // The subscription id is all this event contributes; the plan
            // itself arrives via customer.subscription.created/updated.
            let tenant = session
                .client_reference_id
                .as_deref()
                .and_then(|v| v.parse::<u64>().ok())
                .map(crate::tenant::TenantId);
            let sub_id = session.subscription.as_ref().map(|s| match s {
                stripe_types::Expandable::Id(id) => id.to_string(),
                stripe_types::Expandable::Object(o) => o.id.to_string(),
            });
            match (tenant, sub_id) {
                (Some(tenant), Some(sub_id)) => {
                    if st
                        .store
                        .set_stripe_subscription_id(tenant, &sub_id)
                        .await
                        .is_err()
                    {
                        return fail(st, event_id, StatusCode::SERVICE_UNAVAILABLE).await;
                    }
                }
                _ => {
                    // Not fatal (the session still completed on Stripe's
                    // side, nothing to retry), but worth a signal: a
                    // checkout without a parseable tenant or subscription id
                    // means either an integration bug on our side (missing
                    // `client_reference_id`) or a session created outside
                    // our own checkout flow.
                    tracing::warn!(
                        session_id = session.id.as_str(),
                        "checkout.session.completed missing tenant or subscription id"
                    );
                }
            }
            StatusCode::OK.into_response()
        }
        EventObject::CustomerSubscriptionCreated(sub)
        | EventObject::CustomerSubscriptionUpdated(sub)
        | EventObject::CustomerSubscriptionDeleted(sub) => {
            // Spec D4: never trust the payload's state, fetch the current
            // subscription. Event order is not guaranteed; the API is.
            let fetched = stripe_billing::subscription::RetrieveSubscription::new(sub.id.clone())
                .send(&billing.client)
                .await;
            let current = match fetched {
                Ok(s) => s,
                Err(e) => {
                    tracing::warn!(error = %e, "stripe subscription re-fetch failed");
                    return fail(st, event_id, StatusCode::SERVICE_UNAVAILABLE).await;
                }
            };
            match apply_subscription(&st, &current).await {
                Ok(()) => StatusCode::OK.into_response(),
                Err(status) => fail(st, event_id, status).await,
            }
        }
        _ => {
            // invoice.paid / invoice.payment_failed and anything else we do
            // not act on: structured log only, Stripe's own emails handle
            // dunning communication.
            tracing::info!(event_type = %event.type_, "stripe event acknowledged");
            StatusCode::OK.into_response()
        }
    }
}
