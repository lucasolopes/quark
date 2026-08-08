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
            customer.id.to_string()
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
    let had_subscription = match st.store.get_stripe_subscription_id(tenant).await {
        Ok(v) => v.is_some(),
        Err(_) => return StatusCode::SERVICE_UNAVAILABLE.into_response(),
    };

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
