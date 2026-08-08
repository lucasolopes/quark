//! Plan and lookup-key mapping (LUC-41 phase 2). Pure functions, no IO.
//!
//! Prices in Stripe carry a stable `lookup_key`; price IDs never appear in
//! code or env (spec D3). These names are the contract with the dashboard
//! setup documented in `docs/RUNBOOK-stripe.md`.

use crate::ee::plan::Plan;
use stripe_shared::SubscriptionStatus;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Cycle {
    Monthly,
    Yearly,
}

impl Cycle {
    /// Wire name used in the checkout request body. Named `parse`, not
    /// `from_str`, to keep clippy's `should_implement_trait` quiet without
    /// pulling in a `FromStr` impl nothing needs.
    pub fn parse(s: &str) -> Option<Cycle> {
        match s {
            "monthly" => Some(Cycle::Monthly),
            "yearly" => Some(Cycle::Yearly),
            _ => None,
        }
    }
}

/// The Stripe price lookup key for a plan and cycle. `None` for plans that
/// are not self-service: Free has nothing to buy, Custom is negotiated and
/// set through the operator escape hatch (phase 1).
pub fn lookup_key(plan: Plan, cycle: Cycle) -> Option<&'static str> {
    let key = match (plan, cycle) {
        (Plan::Starter, Cycle::Monthly) => "starter-monthly",
        (Plan::Starter, Cycle::Yearly) => "starter-yearly",
        (Plan::Pro, Cycle::Monthly) => "pro-monthly",
        (Plan::Pro, Cycle::Yearly) => "pro-yearly",
        (Plan::Business, Cycle::Monthly) => "business-monthly",
        (Plan::Business, Cycle::Yearly) => "business-yearly",
        (Plan::Free, _) | (Plan::Custom, _) => return None,
    };
    Some(key)
}

/// Inverts `lookup_key`, cycle-insensitive: the webhook only needs the plan.
pub fn plan_for_lookup_key(key: &str) -> Option<Plan> {
    match key {
        "starter-monthly" | "starter-yearly" => Some(Plan::Starter),
        "pro-monthly" | "pro-yearly" => Some(Plan::Pro),
        "business-monthly" | "business-yearly" => Some(Plan::Business),
        _ => None,
    }
}

/// The dunning table from the spec (D8): what plan a subscription status
/// actually grants. `past_due` keeps the paid plan through the Smart Retries
/// window; every terminal state drops to Free.
pub fn effective_plan(status: &SubscriptionStatus, paid: Plan) -> Plan {
    match status {
        SubscriptionStatus::Active | SubscriptionStatus::Trialing | SubscriptionStatus::PastDue => {
            paid
        }
        _ => Plan::Free,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ee::plan::Plan;
    use stripe_shared::SubscriptionStatus;

    /// Every self-service plan/cycle pair has a key, the key round-trips back
    /// to the plan, and non-purchasable plans have none. The keys are the
    /// contract with the dashboard setup in `docs/RUNBOOK-stripe.md`.
    #[test]
    fn lookup_keys_round_trip_for_self_service_plans() {
        for plan in [Plan::Starter, Plan::Pro, Plan::Business] {
            for cycle in [Cycle::Monthly, Cycle::Yearly] {
                let key = lookup_key(plan, cycle).expect("self-service plan has a key");
                assert_eq!(plan_for_lookup_key(key), Some(plan), "{key}");
            }
        }
        assert_eq!(lookup_key(Plan::Free, Cycle::Monthly), None);
        assert_eq!(lookup_key(Plan::Custom, Cycle::Monthly), None);
        assert_eq!(plan_for_lookup_key("nonsense"), None);
    }

    #[test]
    fn published_key_names_are_stable() {
        assert_eq!(
            lookup_key(Plan::Starter, Cycle::Monthly),
            Some("starter-monthly")
        );
        assert_eq!(
            lookup_key(Plan::Business, Cycle::Yearly),
            Some("business-yearly")
        );
    }

    /// The dunning table from the spec (D8): retries keep the paid plan,
    /// terminal states drop to Free.
    #[test]
    fn effective_plan_follows_the_dunning_table() {
        let paid = Plan::Pro;
        for keeps in [
            SubscriptionStatus::Active,
            SubscriptionStatus::Trialing,
            SubscriptionStatus::PastDue,
        ] {
            assert_eq!(effective_plan(&keeps, paid), Plan::Pro, "{keeps:?}");
        }
        for drops in [
            SubscriptionStatus::Canceled,
            SubscriptionStatus::Unpaid,
            SubscriptionStatus::IncompleteExpired,
            SubscriptionStatus::Paused,
            SubscriptionStatus::Incomplete,
        ] {
            assert_eq!(effective_plan(&drops, paid), Plan::Free, "{drops:?}");
        }
    }
}
