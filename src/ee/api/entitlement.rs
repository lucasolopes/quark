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
pub async fn plan_of(st: &AppState, tenant: TenantId) -> Plan {
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
