//! Stripe billing runtime (LUC-41 phase 2). Covered by `src/ee/LICENSE`.
//!
//! Optional, like the Keycloak runtime: without the three env vars the field
//! stays `None` and the billing endpoints answer 404. A self-hosted
//! Enterprise build without Stripe keeps working in full, which is why the
//! plan layer (phase 1) is independent of the gateway by construction.

pub mod map;

use std::time::Duration;

pub struct StripeBilling {
    pub client: stripe::Client,
    /// The `whsec_...` endpoint secret, for `Webhook::construct_event`.
    pub webhook_secret: String,
    /// Panel base URL without trailing slash, for success/cancel/return URLs.
    pub panel_url: String,
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
        })
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
