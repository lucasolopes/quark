pub(crate) use crate::abuse::{extract_host, is_internal_host};
pub(crate) use crate::analytics::{device_from_ua, AnalyticsSink, ClickEvent};
pub(crate) use crate::auth::{generate_token, hash_token, ApiToken, Scope};
pub(crate) use crate::cache::Cache;
pub(crate) use crate::dns::Dns;
pub(crate) use crate::domain::{Domain, DomainStatus, SHARED_DOMAIN_ID};
pub(crate) use crate::pixel::{PixelConfig, PixelCredentials, Provider};
pub(crate) use crate::sso::{normalize_email_domain, SsoEmailDomain};
pub(crate) use crate::store::{
    matched_rule_index, normalize_folder, normalize_tags, pick_variant, AlertRule, LinkHealth,
    Record, Rule, RuleField, Store, StoreError, Variant,
};
pub(crate) use crate::webhooks::delivery::WebhookDispatcher;
pub(crate) use crate::webhooks::{
    self, EventType, SubscriptionKind, WebhookEvent, WebhookSubscription,
};
pub(crate) use crate::{codec, now, permute};
pub(crate) use axum::body::Bytes;
pub(crate) use axum::extract::{ConnectInfo, Path, Query, RawQuery, Request, State};
pub(crate) use axum::http::Method;
pub(crate) use axum::http::{header, HeaderMap, StatusCode};
pub(crate) use axum::middleware::Next;
pub(crate) use axum::response::{IntoResponse, Response};
pub(crate) use axum::routing::{get, post};
pub(crate) use axum::{Json, Router};
pub(crate) use base64::Engine as _;
pub(crate) use serde::{Deserialize, Serialize};
pub(crate) use std::net::SocketAddr;
pub(crate) use std::sync::atomic::Ordering;
pub(crate) use std::sync::Arc;
pub(crate) use std::time::Instant;
pub(crate) use tower_http::cors::CorsLayer;

pub struct AppState {
    pub cache: Cache,
    pub store: Arc<dyn Store>,
    pub key: u64,
    /// Dedicated 32-byte secret for signing unlock cookies (link passwords).
    /// Kept separate from `key` (the 64-bit code-permutation key) so the MAC
    /// secret has full entropy and no shared purpose with the public codec.
    pub signing_key: [u8; 32],
    pub analytics_tx: tokio::sync::mpsc::Sender<ClickEvent>,
    pub sink: Arc<dyn AnalyticsSink>,
    pub admin_token: Option<String>,
    pub ratelimiter: crate::abuse::ratelimit::RateLimiter,
    pub block_private: bool,
    pub public_host: Option<String>,
    pub real_ip_header: String,
    pub webhooks: Arc<WebhookDispatcher>,
    /// OIDC login runtime, present only when OIDC is configured AND initialized.
    pub oidc: Option<Arc<crate::oidc::OidcRuntime>>,
    /// Whether OIDC was configured at all (`QUARK_OIDC_ISSUER` set), independent
    /// of whether init succeeded. Gates the "public shortener" fallback so a
    /// failed IdP init on an OIDC-only deploy fails closed, not open.
    pub oidc_configured: bool,
    /// Google Sheets connector config, present only when the connector is
    /// opted in (`QUARK_SHEETS_CLIENT_ID`/`_SECRET`/`_REDIRECT_URL` all set).
    pub sheets: Option<Arc<crate::sheets::SheetsConfig>>,
    /// The Sheets HTTP seam (real `GoogleSheetsApi` in `main`, absent in tests
    /// that never drive a real sync). `None` is treated as "connector off".
    pub sheets_api: Option<Arc<dyn crate::sheets::client::SheetsApi>>,
    /// Slack "Add to Slack" connector config, present only when the connector is
    /// opted in (`QUARK_SLACK_CLIENT_ID`/`_SECRET`/`_REDIRECT_URL` all set). The
    /// install persists a `kind: Slack` webhook subscription; there is no
    /// bespoke Slack storage.
    pub slack: Option<Arc<crate::slack::SlackConfig>>,
    /// Multi-tenant (cloud) mode, from `QUARK_MULTI_TENANT`. Gates FORCE RLS,
    /// per-tenant tx, and (P3 Task 4) whether `redirect`/`unlock` resolve the
    /// `Host` header at all: off, they skip straight to the shared route.
    pub multi_tenant: bool,
    /// Maps a request `Host` header to `{domain_id, tenant_id}` for custom
    /// domains (multi-tenancy P3). In OSS/single-tenant mode every host still
    /// resolves through `public_host` to the shared route. `redirect`/`unlock`
    /// consult this (via `resolve_host_route`) to pick the alias domain and
    /// the tenant the link fetch is scoped by.
    pub host_router: Arc<crate::domain_router::HostRouter>,
    /// TXT lookup seam for custom-domain verification (multi-tenancy P3).
    /// Only `admin_domains_verify` calls it; never on the redirect path.
    pub dns: Arc<dyn Dns>,
    /// Base suffix for the auto per-tenant subdomain (multi-tenancy P3-completion),
    /// e.g. `quarkus.com.br` from `QUARK_TENANT_DOMAIN_SUFFIX`. Cloud-only; `None`
    /// disables the whole subdomain-auto feature (no seed on create, no boot
    /// backfill, `/admin/me` reports `null`).
    pub tenant_domain_suffix: Option<String>,
    /// Per-tenant `OidcRuntime` cache (multi-tenancy P2d): each cloud tenant's
    /// own IdP config (`oidc_configs`) is built into a runtime lazily on first
    /// login and cached here, keyed by tenant id. Invalidated (best-effort) by
    /// `admin_oidc_config_put`/`_delete`; also self-expires via TTL.
    pub oidc_tenants: crate::oidc::TenantOidcCache,
    /// Keycloak admin runtime (multi-tenancy P2e), present only when
    /// `QUARK_KEYCLOAK_BASE_URL` is configured. `None` disables the whole
    /// feature; provisioning logic that calls this is Task 2, not built here.
    pub keycloak: Option<Arc<dyn crate::keycloak::KeycloakAdmin>>,
    /// Base URL Keycloak is reachable at, kept alongside `keycloak` so a
    /// tenant's issuer can be derived (`keycloak::derive_issuer`) without
    /// re-reading the environment.
    pub keycloak_base_url: Option<String>,
}

/// Break-glass admin token header, checked by `admin_guard` and allowed through
/// CORS. Kept as a const so the guard, the CSRF check, and the CORS allow-list
/// never drift apart on the literal.
pub(crate) const HEADER_ADMIN_TOKEN: &str = "x-admin-token";
/// Double-submit CSRF header for the cookie-session admin path.
pub(crate) const HEADER_CSRF: &str = "x-quark-csrf";

impl AppState {
    /// Public short code for a link id: permute the id with the instance key,
    /// then base62-encode. The inverse is `permute::decode` (guarded by
    /// `permute::MAX_ID`). Centralizes the composition that the create/redirect
    /// and admin-list paths would otherwise repeat.
    pub(crate) fn encode_code(&self, id: u64) -> String {
        codec::to_base62(permute::encode(id, self.key))
    }
}

mod domains;
mod guard;
mod invites;
mod links;
mod links_admin;
mod oidc_login;
mod router;
mod sheets;
mod slack;
mod sso_domains;
mod tenants;
mod webhooks_api;

pub(crate) use domains::*;
pub use guard::*;
pub(crate) use invites::*;
pub use links::*;
pub(crate) use links_admin::*;
pub(crate) use oidc_login::*;
pub use router::*;
pub(crate) use sheets::*;
pub(crate) use slack::*;
pub(crate) use sso_domains::*;
pub use tenants::*;
pub(crate) use webhooks_api::*;

#[cfg(test)]
mod tests;
