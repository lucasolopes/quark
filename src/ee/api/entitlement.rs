//! Plan enforcement (LUC-41 phase 1). Covered by `src/ee/LICENSE`, not the AGPL.
//!
//! The Enterprise half of `api/entitlement.rs`: reads the tenant's plan and
//! answers against the catalog in `crate::ee::plan`.

use crate::api::entitlement::{Denied, Feature, Quota};
use crate::api::AppState;
use crate::ee::plan::Plan;
use crate::tenant::TenantId;

/// The tenant's plan, through the cache.
///
/// A store error resolves to `Free` for THIS request only, and is not written
/// to the cache: a blip in the plan lookup must not take a paying tenant's
/// product down for up to `PLAN_TTL_SECS`, so the next request tries the
/// store again instead of being stuck denied. `Ok(None)` (no plan row yet) is
/// a legitimate answer from the store, not an error, so it IS cached as
/// `Free` like any other resolved plan.
///
/// A backend that does not support plans at all (`Store::supports_plans() ==
/// false`, i.e. LMDB) is a different case from "no plan row yet" and is
/// handled before either of the above: a store with no plan system cannot
/// place a tenant on a plan, so treating its `Ok(None)` the same as Postgres's
/// "not signed up" would deny an Enterprise self-hosted install (embedded
/// store, `--features ee`) every feature it already paid for. The product
/// decision is that "no plan system" means UNLIMITED, not `Free`: this
/// answers `Plan::Custom` (unlimited across the board) without a store call
/// or a cache write, since the answer is a static property of the backend
/// and never changes at runtime.
pub async fn plan_of(st: &AppState, tenant: TenantId) -> Plan {
    if !st.store.supports_plans() {
        return Plan::Custom;
    }
    if let Some(p) = st.ee.plans.get(tenant).await {
        return p;
    }
    match st.store.get_tenant_plan(tenant).await {
        Ok(Some(s)) => {
            let p = Plan::from_stored(&s);
            st.ee.plans.put(tenant, p).await;
            p
        }
        Ok(None) => {
            st.ee.plans.put(tenant, Plan::Free).await;
            Plan::Free
        }
        Err(_) => Plan::Free,
    }
}

pub async fn require(st: &AppState, tenant: TenantId, f: Feature) -> Result<(), Denied> {
    if plan_of(st, tenant).await.allows(f) {
        return Ok(());
    }
    Err(Denied {
        limit: f.as_str(),
        allowed: None,
        upgrade_to: Plan::cheapest_with(f).map(Plan::as_str).unwrap_or("custom"),
    })
}

/// `current` is how many the tenant already holds. The call is made BEFORE
/// creating the next one, so the check is `current >= ceiling`.
pub async fn require_quota(
    st: &AppState,
    tenant: TenantId,
    q: Quota,
    current: u64,
) -> Result<(), Denied> {
    let plan = plan_of(st, tenant).await;
    let limits = plan.limits();
    let ceiling = match q {
        Quota::Domains => limits.domains,
        Quota::Members => limits.members,
    };
    let Some(ceiling) = ceiling else {
        return Ok(()); // unlimited
    };
    if current < u64::from(ceiling) {
        return Ok(());
    }
    Err(Denied {
        limit: q.as_str(),
        allowed: Some(u64::from(ceiling)),
        upgrade_to: cheapest_above(q, u64::from(ceiling)),
    })
}

/// Cheapest plan whose ceiling for `q` is above `ceiling` (or unlimited).
fn cheapest_above(q: Quota, ceiling: u64) -> &'static str {
    Plan::ALL
        .into_iter()
        .find(|p| {
            let l = p.limits();
            let c = match q {
                Quota::Domains => l.domains,
                Quota::Members => l.members,
            };
            c.is_none_or(|c| u64::from(c) > ceiling)
        })
        .map(Plan::as_str)
        .unwrap_or("custom")
}

use crate::api::{admin_guard, constant_time_eq};
use crate::auth::Scope;
use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::Json;
use std::sync::Arc;

/// `GET /admin/plan`: the tenant's plan, its ceilings and its unlocked
/// features.
///
/// The panel renders from this instead of carrying its own copy of the grid,
/// which would drift on the first change.
pub(crate) async fn admin_plan_get(
    State(st): State<Arc<AppState>>,
    headers: HeaderMap,
) -> Response {
    let p = match admin_guard(&st, &headers, Scope::LinksRead).await {
        Ok(p) => p,
        Err(status) => return status.into_response(),
    };
    let plan = plan_of(&st, p.tenant).await;
    let l = plan.limits();
    let features: Vec<&'static str> = Feature::ALL
        .into_iter()
        .filter(|f| plan.allows(*f))
        .map(Feature::as_str)
        .collect();
    Json(serde_json::json!({
        "plan": plan.as_str(),
        "limits": {
            "domains": l.domains,
            "members": l.members,
            "automation_per_month": l.automation_per_month,
            "tracked_clicks_per_month": l.tracked_clicks_per_month,
            "retention_days": l.retention_days,
        },
        "features": features,
    }))
    .into_response()
}

#[derive(serde::Deserialize)]
pub(crate) struct SetPlanReq {
    pub plan: String,
}

/// `PUT /admin/tenants/{id}/plan`: operator-only plan change.
///
/// Requires the break-glass `QUARK_ADMIN_TOKEN` directly, compared in
/// constant time, and NOT a tenant API token or session. `admin_guard`
/// resolving a `Scope::Full` API token would still be a credential a tenant
/// controls itself. `Plan::Custom` grants everything unlimited and `"custom"`
/// is a string the parser recognizes, so anyone who could write this column
/// would grant themselves unrestricted access; only the operator's own
/// break-glass token may do it. Phase 2 replaces the manual call with the
/// Stripe webhook, and this endpoint stays as the operator escape hatch.
///
/// The `st.ee.plans.invalidate(tenant)` call below makes the new plan take
/// effect immediately, but only on the process that handled THIS request:
/// `PlanCache` is per-process, with no cross-node invalidation. On a
/// multi-replica deploy the other nodes keep serving the old plan out of
/// their own cache until it converges on its own, within `PLAN_TTL_SECS`
/// (60s). Wiring cross-node invalidation (e.g. over the pub/sub channel
/// `src/invalidate.rs` already uses) is future work, not done here.
pub(crate) async fn admin_tenant_plan_put(
    State(st): State<Arc<AppState>>,
    Path(id): Path<u64>,
    headers: HeaderMap,
    Json(req): Json<SetPlanReq>,
) -> Response {
    let provided = headers
        .get(crate::api::HEADER_ADMIN_TOKEN)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");
    let ok = st
        .admin_token
        .as_deref()
        .is_some_and(|expected| constant_time_eq(provided.as_bytes(), expected.as_bytes()));
    if !ok {
        return StatusCode::UNAUTHORIZED.into_response();
    }
    let plan = Plan::from_stored(&req.plan);
    // Reject a typo instead of silently downgrading the tenant to Free:
    // `from_stored` falls back to `Free` on any unrecognized string, which is
    // safe on read but would rewrite the tenant's plan on a write typo.
    if plan.as_str() != req.plan {
        return (StatusCode::BAD_REQUEST, "unknown plan").into_response();
    }
    let tenant = TenantId(id);
    if let Err(_e) = st.store.set_tenant_plan(tenant, plan.as_str()).await {
        return StatusCode::SERVICE_UNAVAILABLE.into_response();
    }
    st.ee.plans.invalidate(tenant).await;
    StatusCode::NO_CONTENT.into_response()
}
