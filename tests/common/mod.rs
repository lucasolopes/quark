//! Shared test helpers for the integration-test crates.
//!
//! Rust integration tests are separate crates, so this module is included by
//! each `tests/*.rs` that wants it via `mod common;`. Not every test uses every
//! helper, so `#![allow(dead_code)]` keeps `-D warnings` happy in the crates
//! that only touch a subset.
#![allow(dead_code)]

use std::sync::atomic::AtomicBool;
use std::sync::Arc;

use quark::abuse::ratelimit::RateLimiter;
use quark::analytics::{AnalyticsSink, ClickEvent};
use quark::api::{AppState, DEFAULT_REAL_IP_HEADER};
use quark::cache::Cache;
use quark::dns::{Dns, NullDns};
use quark::domain_router::HostRouter;
use quark::keycloak::KeycloakAdmin;
use quark::sheets::SheetsConfig;
use quark::slack::SlackConfig;
use quark::store::Store;
use quark::webhooks::delivery::WebhookDispatcher;

/// A `WebhookDispatcher` for tests that don't exercise webhooks: the receiver
/// is dropped immediately, so `emit` silently no-ops (logs and drops) rather
/// than needing a live worker. Matches the per-file `test_webhook_dispatcher`
/// helpers the tests used before this builder existed.
pub fn test_webhook_dispatcher() -> Arc<WebhookDispatcher> {
    let (tx, _rx) = tokio::sync::mpsc::channel(1);
    Arc::new(WebhookDispatcher::new(
        tx,
        Arc::new(AtomicBool::new(false)),
        Arc::new(AtomicBool::new(false)),
    ))
}

/// Fluent builder for an `Arc<AppState>` in tests.
///
/// `new(store, sink)` seeds the common OSS single-tenant shape; every field a
/// test currently customizes has a setter. The `cache`, `host_router`,
/// `analytics_tx`, and `webhooks` wiring inputs default to the standard test
/// wiring derived from `store`, and can be overridden for tests that build
/// their own (e.g. a real `Invalidator`, a live analytics receiver, or a real
/// webhook dispatcher).
pub struct TestState {
    store: Arc<dyn Store>,
    sink: Arc<dyn AnalyticsSink>,
    cache: Option<Cache>,
    host_router: Option<Arc<HostRouter>>,
    analytics_tx: Option<tokio::sync::mpsc::Sender<ClickEvent>>,
    webhooks: Option<Arc<WebhookDispatcher>>,
    key: u64,
    signing_key: [u8; 32],
    admin_token: Option<String>,
    ratelimiter: RateLimiter,
    block_private: bool,
    public_host: Option<String>,
    real_ip_header: String,
    oidc_configured: bool,
    sheets: Option<Arc<SheetsConfig>>,
    slack: Option<Arc<SlackConfig>>,
    multi_tenant: bool,
    tenant_domain_suffix: Option<String>,
    keycloak: Option<Arc<dyn KeycloakAdmin>>,
    keycloak_base_url: Option<String>,
    dns: Arc<dyn Dns>,
}

impl TestState {
    pub fn new(store: Arc<dyn Store>, sink: Arc<dyn AnalyticsSink>) -> Self {
        TestState {
            store,
            sink,
            cache: None,
            host_router: None,
            analytics_tx: None,
            webhooks: None,
            key: 0x1234,
            signing_key: [0u8; 32],
            admin_token: None,
            ratelimiter: RateLimiter::disabled(),
            block_private: true,
            public_host: None,
            real_ip_header: DEFAULT_REAL_IP_HEADER.to_string(),
            oidc_configured: false,
            sheets: None,
            slack: None,
            multi_tenant: false,
            tenant_domain_suffix: None,
            keycloak: None,
            keycloak_base_url: None,
            dns: Arc::new(NullDns),
        }
    }

    pub fn cache(mut self, cache: Cache) -> Self {
        self.cache = Some(cache);
        self
    }

    pub fn host_router(mut self, host_router: Arc<HostRouter>) -> Self {
        self.host_router = Some(host_router);
        self
    }

    pub fn analytics_tx(mut self, tx: tokio::sync::mpsc::Sender<ClickEvent>) -> Self {
        self.analytics_tx = Some(tx);
        self
    }

    pub fn webhooks(mut self, webhooks: Arc<WebhookDispatcher>) -> Self {
        self.webhooks = Some(webhooks);
        self
    }

    pub fn key(mut self, key: u64) -> Self {
        self.key = key;
        self
    }

    pub fn signing_key(mut self, signing_key: [u8; 32]) -> Self {
        self.signing_key = signing_key;
        self
    }

    pub fn admin_token(mut self, admin_token: Option<String>) -> Self {
        self.admin_token = admin_token;
        self
    }

    pub fn ratelimiter(mut self, ratelimiter: RateLimiter) -> Self {
        self.ratelimiter = ratelimiter;
        self
    }

    pub fn block_private(mut self, block_private: bool) -> Self {
        self.block_private = block_private;
        self
    }

    pub fn public_host(mut self, public_host: Option<String>) -> Self {
        self.public_host = public_host;
        self
    }

    pub fn real_ip_header(mut self, real_ip_header: String) -> Self {
        self.real_ip_header = real_ip_header;
        self
    }

    pub fn oidc_configured(mut self, oidc_configured: bool) -> Self {
        self.oidc_configured = oidc_configured;
        self
    }

    pub fn sheets(mut self, sheets: Option<Arc<SheetsConfig>>) -> Self {
        self.sheets = sheets;
        self
    }

    pub fn slack(mut self, slack: Option<Arc<SlackConfig>>) -> Self {
        self.slack = slack;
        self
    }

    pub fn multi_tenant(mut self, multi_tenant: bool) -> Self {
        self.multi_tenant = multi_tenant;
        self
    }

    pub fn tenant_domain_suffix(mut self, tenant_domain_suffix: Option<String>) -> Self {
        self.tenant_domain_suffix = tenant_domain_suffix;
        self
    }

    pub fn keycloak(mut self, keycloak: Option<Arc<dyn KeycloakAdmin>>) -> Self {
        self.keycloak = keycloak;
        self
    }

    pub fn keycloak_base_url(mut self, keycloak_base_url: Option<String>) -> Self {
        self.keycloak_base_url = keycloak_base_url;
        self
    }

    pub fn dns(mut self, dns: Arc<dyn Dns>) -> Self {
        self.dns = dns;
        self
    }

    pub fn build(self) -> Arc<AppState> {
        let cache = self
            .cache
            .unwrap_or_else(|| Cache::new(self.store.clone(), 1000, None));
        let host_router = self
            .host_router
            .unwrap_or_else(|| Arc::new(HostRouter::new(self.store.clone(), None, None)));
        let analytics_tx = self.analytics_tx.unwrap_or_else(|| {
            let (tx, _rx) = tokio::sync::mpsc::channel(100);
            tx
        });
        let webhooks = self.webhooks.unwrap_or_else(test_webhook_dispatcher);
        Arc::new(AppState {
            cache,
            store: self.store,
            key: self.key,
            signing_key: self.signing_key,
            analytics_tx,
            sink: self.sink,
            admin_token: self.admin_token,
            ratelimiter: self.ratelimiter,
            block_private: self.block_private,
            public_host: self.public_host,
            real_ip_header: self.real_ip_header,
            webhooks,
            oidc: None,
            oidc_configured: self.oidc_configured,
            sheets: self.sheets,
            sheets_api: None,
            slack: self.slack,
            multi_tenant: self.multi_tenant,
            host_router,
            dns: self.dns,
            tenant_domain_suffix: self.tenant_domain_suffix,
            oidc_tenants: quark::oidc::TenantOidcCache::new(),
            keycloak: self.keycloak,
            keycloak_base_url: self.keycloak_base_url,
        })
    }
}
