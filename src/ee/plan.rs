//! The plan catalog. Covered by `src/ee/LICENSE`, not by the AGPL.
//!
//! The numbers are the published grid in
//! `docs/DECISAO-planos-e-pricing-cloud.md`. They live in code, versioned
//! alongside the features they limit, so changing a limit is a deploy and
//! applies at once to everyone on that plan.

use crate::api::entitlement::Feature;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Plan {
    Free,
    Starter,
    Pro,
    Business,
    Custom,
}

/// Numeric ceilings. `None` means unlimited.
///
/// Deliberately does NOT implement `Default`: adding a field must force every
/// plan below to state a value, instead of silently inheriting a zero.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Limits {
    pub domains: Option<u32>,
    pub members: Option<u32>,
    pub automation_per_month: Option<u64>,
    pub tracked_clicks_per_month: Option<u64>,
    pub retention_days: Option<u32>,
}

impl Plan {
    /// Strictly increasing in price: `Free` cheapest, `Custom` most
    /// expensive. `Plan::cheapest_with` returns the first match in this
    /// order, and the `feature_access_is_monotonic_up_the_ladder` test walks
    /// it assuming later entries are pricier. Reordering this array (e.g. to
    /// insert a new tier in the middle) silently breaks both without a
    /// compile error — see `plan_all_is_in_ascending_price_order` below,
    /// which pins the exact sequence so a reorder fails loudly instead.
    pub const ALL: [Plan; 5] = [
        Plan::Free,
        Plan::Starter,
        Plan::Pro,
        Plan::Business,
        Plan::Custom,
    ];

    /// Parses the opaque string the store keeps. An unknown value falls back to
    /// `Free` rather than failing the request: a typo in the column must not
    /// hand out a better plan, and must not take the product down either.
    pub fn from_stored(s: &str) -> Plan {
        match s {
            "starter" => Plan::Starter,
            "pro" => Plan::Pro,
            "business" => Plan::Business,
            "custom" => Plan::Custom,
            _ => Plan::Free,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Plan::Free => "free",
            Plan::Starter => "starter",
            Plan::Pro => "pro",
            Plan::Business => "business",
            Plan::Custom => "custom",
        }
    }

    /// Whether this plan unlocks `f`.
    ///
    /// Nested exhaustive matches with NO wildcard arm, on purpose: adding a
    /// variant to `Feature` breaks the build here and lists every plan that
    /// still has to decide. A list of allowed features would instead fail
    /// silently, leaving the new feature denied everywhere.
    pub fn allows(self, f: Feature) -> bool {
        match self {
            Plan::Free => match f {
                Feature::Webhooks => false,
                Feature::Integrations => false,
                Feature::HealthMonitoring => false,
                Feature::TokenScopes => false,
                Feature::Sso => false,
            },
            Plan::Starter => match f {
                Feature::Webhooks => true,
                Feature::Integrations => true,
                Feature::HealthMonitoring => false,
                Feature::TokenScopes => false,
                Feature::Sso => false,
            },
            Plan::Pro => match f {
                Feature::Webhooks => true,
                Feature::Integrations => true,
                Feature::HealthMonitoring => true,
                Feature::TokenScopes => true,
                Feature::Sso => false,
            },
            Plan::Business => match f {
                Feature::Webhooks => true,
                Feature::Integrations => true,
                Feature::HealthMonitoring => true,
                Feature::TokenScopes => true,
                Feature::Sso => true,
            },
            Plan::Custom => match f {
                Feature::Webhooks => true,
                Feature::Integrations => true,
                Feature::HealthMonitoring => true,
                Feature::TokenScopes => true,
                Feature::Sso => true,
            },
        }
    }

    pub fn limits(self) -> Limits {
        match self {
            Plan::Free => Limits {
                domains: Some(3),
                members: Some(1),
                automation_per_month: Some(100),
                tracked_clicks_per_month: Some(50_000),
                retention_days: Some(30),
            },
            Plan::Starter => Limits {
                domains: Some(10),
                members: Some(3),
                automation_per_month: Some(5_000),
                tracked_clicks_per_month: Some(250_000),
                retention_days: Some(365),
            },
            Plan::Pro => Limits {
                domains: Some(50),
                members: Some(10),
                automation_per_month: Some(50_000),
                tracked_clicks_per_month: Some(1_000_000),
                retention_days: Some(730),
            },
            Plan::Business => Limits {
                domains: None,
                members: None,
                automation_per_month: Some(500_000),
                tracked_clicks_per_month: Some(5_000_000),
                retention_days: Some(1_095),
            },
            // Negotiated. The per-tenant override narrows this; absent an
            // override, Custom is unlimited.
            Plan::Custom => Limits {
                domains: None,
                members: None,
                automation_per_month: None,
                tracked_clicks_per_month: None,
                retention_days: None,
            },
        }
    }

    /// The cheapest plan that unlocks `f`, for the upgrade hint in a `402`.
    pub fn cheapest_with(f: Feature) -> Option<Plan> {
        Plan::ALL.into_iter().find(|p| p.allows(f))
    }
}

/// Per-tenant plan cache. The plan is read on every gated request, and the
/// store round-trip would otherwise be paid each time. Same shape the crate
/// already uses for `TenantOidcCache` and the host router: moka with a TTL,
/// plus explicit invalidation when the plan changes.
#[derive(Clone)]
pub struct PlanCache {
    cache: moka::future::Cache<crate::tenant::TenantId, Plan>,
}

/// Short enough that a plan change nobody invalidated still converges quickly,
/// long enough that the store is not hit per request.
const PLAN_TTL_SECS: u64 = 60;

impl PlanCache {
    pub fn new() -> PlanCache {
        PlanCache {
            cache: moka::future::Cache::builder()
                .time_to_live(std::time::Duration::from_secs(PLAN_TTL_SECS))
                .build(),
        }
    }

    pub async fn get(&self, tenant: crate::tenant::TenantId) -> Option<Plan> {
        self.cache.get(&tenant).await
    }

    pub async fn put(&self, tenant: crate::tenant::TenantId, plan: Plan) {
        self.cache.insert(tenant, plan).await;
    }

    pub async fn invalidate(&self, tenant: crate::tenant::TenantId) {
        self.cache.invalidate(&tenant).await;
    }
}

impl Default for PlanCache {
    fn default() -> Self {
        PlanCache::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::entitlement::Feature;

    /// Every feature a cheaper plan allows, a more expensive plan must also
    /// allow. A hole here means someone downgraded a tier by accident.
    #[test]
    fn feature_access_is_monotonic_up_the_ladder() {
        for f in Feature::ALL {
            let mut seen_allowed = false;
            for p in Plan::ALL {
                let allowed = p.allows(f);
                if seen_allowed {
                    assert!(allowed, "{p:?} denies {f:?} but a cheaper plan allows it");
                }
                seen_allowed |= allowed;
            }
        }
    }

    /// The numbers here are the contract in
    /// `docs/DECISAO-planos-e-pricing-cloud.md`. Changing one is a product
    /// decision, so it has to break this test on the way.
    #[test]
    fn limits_match_the_published_grid() {
        assert_eq!(Plan::Free.limits().domains, Some(3));
        assert_eq!(Plan::Free.limits().members, Some(1));
        assert_eq!(Plan::Starter.limits().domains, Some(10));
        assert_eq!(Plan::Starter.limits().members, Some(3));
        assert_eq!(Plan::Pro.limits().domains, Some(50));
        assert_eq!(Plan::Pro.limits().members, Some(10));
        assert_eq!(Plan::Business.limits().domains, None);
        assert_eq!(Plan::Custom.limits().members, None);
    }

    #[test]
    fn unknown_stored_value_falls_back_to_free() {
        assert_eq!(Plan::from_stored("free"), Plan::Free);
        assert_eq!(Plan::from_stored("pro"), Plan::Pro);
        assert_eq!(Plan::from_stored("nonsense"), Plan::Free);
    }

    #[test]
    fn cheapest_plan_with_a_feature_is_reported_for_the_upgrade_hint() {
        assert_eq!(Plan::cheapest_with(Feature::Webhooks), Some(Plan::Starter));
        assert_eq!(Plan::cheapest_with(Feature::Sso), Some(Plan::Business));
    }

    /// `feature_access_is_monotonic_up_the_ladder` walks `Plan::ALL` and
    /// trusts that it is already in ascending price order; it cannot detect
    /// a reorder because it uses `Plan::ALL` as its own reference. This test
    /// pins the literal sequence instead, so inserting a tier in the wrong
    /// spot (or otherwise reordering the array) fails here immediately, with
    /// a message that says exactly what moved.
    #[test]
    fn plan_all_is_in_ascending_price_order() {
        assert_eq!(
            Plan::ALL,
            [
                Plan::Free,
                Plan::Starter,
                Plan::Pro,
                Plan::Business,
                Plan::Custom,
            ]
        );
    }
}
