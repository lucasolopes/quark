//! Seam for plan enforcement (LUC-41 phase 1).
//!
//! Plans only exist when operating quark as a service for other people, so the
//! catalog and the real check live in `src/ee/`. What the core keeps is this
//! seam: the vocabulary (`Feature`, `Quota`, `Denied`) plus Community
//! implementations that allow everything.
//!
//! The Community edition MUST never enforce a limit. A self-hosted AGPL install
//! is free and unlimited; limiting it would contradict the open-core decision in
//! `docs/specs/2026-08-03-luc19-open-core-design.md`.
//!
//! Two `cfg`-selected functions rather than a trait object: the choice is made
//! at compile time and never varies at runtime.

#[cfg(not(feature = "ee"))]
use crate::tenant::TenantId;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Json;

/// A capability a plan either unlocks or does not. Binary, no ceiling.
///
/// `HealthMonitoring` and `TokenScopes` are deliberately NOT here yet: the
/// commercial grid in `docs/PLANS.md` lists "broken-link monitoring" and
/// "scoped API tokens" as future plan-gated rows, but no handler in this
/// phase actually checks either — `GET /admin/plan` would otherwise advertise
/// them as unlocked while the backend never enforces or even offers them,
/// which the panel would render as real. They come back together with the
/// slice of work that wires them to a real handler.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Feature {
    Webhooks,
    Integrations,
    Sso,
}

impl Feature {
    pub const ALL: [Feature; 3] = [Feature::Webhooks, Feature::Integrations, Feature::Sso];

    /// Stable wire name, used in the `402` body and by the panel.
    pub fn as_str(self) -> &'static str {
        match self {
            Feature::Webhooks => "webhooks",
            Feature::Integrations => "integrations",
            Feature::Sso => "sso",
        }
    }
}

/// A countable ceiling. Phase 1 covers only the ones answerable with a row
/// count; the monthly counters (automation, tracked clicks) arrive in phase 3.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Quota {
    Domains,
    Members,
}

impl Quota {
    pub const ALL: [Quota; 2] = [Quota::Domains, Quota::Members];

    pub fn as_str(self) -> &'static str {
        match self {
            Quota::Domains => "domains",
            Quota::Members => "members",
        }
    }
}

/// Why the request was refused, and what fixes it.
///
/// Renders as `402 Payment Required`, not `403`: the caller does have
/// permission, what is missing is plan.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Denied {
    /// `Feature::as_str` or `Quota::as_str`.
    pub limit: &'static str,
    /// The ceiling that was hit. `None` for a binary feature.
    pub allowed: Option<u64>,
    /// Cheapest plan that lifts it, so the panel can build the upgrade call
    /// without guessing.
    pub upgrade_to: &'static str,
}

impl IntoResponse for Denied {
    fn into_response(self) -> Response {
        (
            StatusCode::PAYMENT_REQUIRED,
            Json(serde_json::json!({
                "error": "plan_limit_reached",
                "limit": self.limit,
                "allowed": self.allowed,
                "upgrade_to": self.upgrade_to,
            })),
        )
            .into_response()
    }
}

/// Community: every feature is allowed.
#[cfg(not(feature = "ee"))]
pub async fn require(_st: &super::AppState, _tenant: TenantId, _f: Feature) -> Result<(), Denied> {
    Ok(())
}

/// Community: no ceiling applies.
#[cfg(not(feature = "ee"))]
pub async fn require_quota(
    _st: &super::AppState,
    _tenant: TenantId,
    _q: Quota,
    _current: u64,
) -> Result<(), Denied> {
    Ok(())
}

#[cfg(feature = "ee")]
pub use crate::ee::api::entitlement::{require, require_quota};
