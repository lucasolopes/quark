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
/// A store error resolves to `Free` rather than failing the request: a blip in
/// the plan lookup must not take a paying tenant's product down, and `Free` is
/// the safe direction (it can only deny, never hand out a better plan).
pub async fn plan_of(st: &AppState, tenant: TenantId) -> Plan {
    if let Some(p) = st.ee.plans.get(tenant).await {
        return p;
    }
    let p = match st.store.get_tenant_plan(tenant).await {
        Ok(Some(s)) => Plan::from_stored(&s),
        Ok(None) => Plan::Free,
        Err(_) => Plan::Free,
    };
    st.ee.plans.put(tenant, p).await;
    p
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
