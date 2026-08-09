//! Stripe billing runtime (LUC-41 phase 2). Covered by `src/ee/LICENSE`.
//!
//! Optional, like the Keycloak runtime: without the three env vars the field
//! stays `None` and the billing endpoints answer 404. A self-hosted
//! Enterprise build without Stripe keeps working in full, which is why the
//! plan layer (phase 1) is independent of the gateway by construction.

pub mod map;

use crate::ee::plan::Plan;
use std::collections::HashMap;
use std::time::{Duration, Instant};

/// One plan's Stripe prices for the catalog, cents per currency and cycle.
#[derive(Debug, Clone, serde::Serialize)]
pub struct CatalogPrice {
    pub usd_cents: i64,
    pub brl_cents: i64,
}

/// The 6 self-service prices, cached process-wide (not per tenant: the grid
/// itself is the same for everyone, only the checkout currency choice and the
/// locked currency are tenant-specific).
#[derive(Debug, Clone)]
pub struct CatalogPrices {
    /// lookup_key -> price. Missing key means the dashboard lacks that price,
    /// or the price exists but is missing a BRL `currency_options` entry (a
    /// half-configured price is treated as absent rather than USD-only, so
    /// the panel's "no price" state is unambiguous).
    pub by_lookup_key: HashMap<String, CatalogPrice>,
    /// The tenant-independent part only; the customer's locked currency is
    /// resolved per request, not cached here.
    pub fetched_at: Instant,
}

/// Cache lifetime for both the catalog prices and the per-tenant locked
/// currency: prices and a customer's currency both change rarely enough that
/// a Stripe API call per catalog view would be wasteful, and a stale value
/// stays useful far longer than a checkout flow's usual timescale (spec D2).
pub const CATALOG_TTL: Duration = Duration::from_secs(12 * 60 * 60);

pub struct StripeBilling {
    pub client: stripe::Client,
    /// The `whsec_...` endpoint secret, for `Webhook::construct_event`.
    pub webhook_secret: String,
    /// Panel base URL without trailing slash, for success/cancel/return URLs.
    pub panel_url: String,
    /// The plan grid's prices, refreshed at most once per `CATALOG_TTL`.
    pub catalog_cache: tokio::sync::RwLock<Option<CatalogPrices>>,
    /// Per-tenant Stripe-locked currency, same TTL and stale-serves-old
    /// pattern as `catalog_cache`. Kept here rather than a crate-wide moka
    /// cache: it is low-traffic (one lookup per catalog view) and belongs
    /// next to the price cache it is displayed alongside.
    pub currency_cache:
        tokio::sync::RwLock<HashMap<crate::tenant::TenantId, (Instant, Option<String>)>>,
}

impl StripeBilling {
    /// Reads `QUARK_STRIPE_SECRET_KEY`, `QUARK_STRIPE_WEBHOOK_SECRET` and
    /// `QUARK_STRIPE_PANEL_URL`. All three or nothing.
    pub fn from_env() -> Option<StripeBilling> {
        Self::from_parts(
            &std::env::var("QUARK_STRIPE_SECRET_KEY").unwrap_or_default(),
            &std::env::var("QUARK_STRIPE_WEBHOOK_SECRET").unwrap_or_default(),
            &std::env::var("QUARK_STRIPE_PANEL_URL").unwrap_or_default(),
            None,
        )
    }

    /// Explicit parts, mirroring `KeycloakConfig::from_parts` so tests never
    /// mutate process env. `api_base` overrides the API URL for tests that
    /// stand up a local mock server.
    pub fn from_parts(
        secret_key: &str,
        webhook_secret: &str,
        panel_url: &str,
        api_base: Option<&str>,
    ) -> Option<StripeBilling> {
        if secret_key.trim().is_empty()
            || webhook_secret.trim().is_empty()
            || panel_url.trim().is_empty()
        {
            return None;
        }
        let mut builder = stripe::ClientBuilder::new(secret_key.trim())
            .request_strategy(stripe::RequestStrategy::Retry(2))
            .timeout(Duration::from_secs(15));
        if let Some(base) = api_base {
            builder = builder.url(base);
        }
        let client = match builder.build() {
            Ok(c) => c,
            Err(e) => {
                tracing::warn!(error = %e, "stripe billing disabled: client build failed");
                return None;
            }
        };
        Some(StripeBilling {
            client,
            webhook_secret: webhook_secret.trim().to_string(),
            panel_url: panel_url.trim().trim_end_matches('/').to_string(),
            catalog_cache: tokio::sync::RwLock::new(None),
            currency_cache: tokio::sync::RwLock::new(HashMap::new()),
        })
    }

    /// The 6 self-service prices, cached for 12h. On a Stripe failure past
    /// the TTL the stale value is served instead of breaking the grid (spec
    /// D2); `None` only when there has never been a successful fetch.
    pub async fn catalog_prices(&self) -> Option<CatalogPrices> {
        {
            let cached = self.catalog_cache.read().await;
            if let Some(cp) = cached.as_ref() {
                if cp.fetched_at.elapsed() < CATALOG_TTL {
                    return Some(cp.clone());
                }
            }
        }

        let keys: Vec<String> = [
            (Plan::Starter, map::Cycle::Monthly),
            (Plan::Starter, map::Cycle::Yearly),
            (Plan::Pro, map::Cycle::Monthly),
            (Plan::Pro, map::Cycle::Yearly),
            (Plan::Business, map::Cycle::Monthly),
            (Plan::Business, map::Cycle::Yearly),
        ]
        .into_iter()
        .filter_map(|(plan, cycle)| map::lookup_key(plan, cycle))
        .map(str::to_string)
        .collect();

        let result = stripe_product::price::ListPrice::new()
            .lookup_keys(keys)
            .active(true)
            .expand(vec!["data.currency_options".to_string()])
            .send(&self.client)
            .await;

        match result {
            Ok(list) => {
                let mut by_lookup_key = HashMap::new();
                for price in list.data {
                    let Some(key) = price.lookup_key.clone() else {
                        continue;
                    };
                    let Some(usd_cents) = price.unit_amount else {
                        continue;
                    };
                    let Some(brl_cents) = price
                        .currency_options
                        .as_ref()
                        .and_then(|opts| opts.get(&stripe_types::Currency::BRL))
                        .and_then(|opt| opt.unit_amount)
                    else {
                        continue;
                    };
                    by_lookup_key.insert(
                        key,
                        CatalogPrice {
                            usd_cents,
                            brl_cents,
                        },
                    );
                }
                let fresh = CatalogPrices {
                    by_lookup_key,
                    fetched_at: Instant::now(),
                };
                *self.catalog_cache.write().await = Some(fresh.clone());
                Some(fresh)
            }
            Err(e) => {
                tracing::warn!(error = %e, "stripe catalog price list failed");
                self.catalog_cache.read().await.clone()
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Billing is all-or-nothing: a partially configured env must resolve to
    /// disabled instead of a half-working checkout.
    #[test]
    fn from_parts_requires_all_three_values() {
        assert!(
            StripeBilling::from_parts("", "whsec_x", "https://app.example.com", None).is_none()
        );
        assert!(
            StripeBilling::from_parts("sk_test_x", "", "https://app.example.com", None).is_none()
        );
        assert!(StripeBilling::from_parts("sk_test_x", "whsec_x", "", None).is_none());
        let b = StripeBilling::from_parts("sk_test_x", "whsec_x", "https://app.example.com/", None)
            .unwrap();
        // Trailing slash is normalized so URL building can always join with '/'.
        assert_eq!(b.panel_url, "https://app.example.com");
    }
}
