//! Maps an incoming `Host` header to a route (`domain_id`, `tenant_id`) for the
//! public redirect hot path. The shared host (`public_host`) always resolves to
//! `SHARED_DOMAIN_ID`/`DEFAULT_TENANT`; a `Verified` custom domain resolves to
//! its own tenant; anything else (unknown host, or a domain still `Pending`
//! verification) resolves to `None` so the caller 404s.
//!
//! v1 is L1-only: a `moka` cache in front of the bare-pool
//! `Store::get_domain_by_host`, caching negatives too (short TTL) so an unknown
//! host cannot hammer the DB. There is no L2 tier yet, unlike `cache::Cache` —
//! domain lookups are much lower volume than link redirects (one per host, not
//! per click), so the TTL alone is enough to keep the store off the hot path.
//! An L2 (Valkey) tier can be added later following the same `CacheTier` +
//! `Breaker` + `L2_OP_TIMEOUT` pattern if custom-domain traffic grows.
use crate::domain::{DomainRoute, DomainStatus};
use crate::invalidate::Invalidator;
use crate::store::Store;
use crate::tenant::DEFAULT_TENANT;
use moka::sync::Cache as Moka;
use std::sync::Arc;

/// How long a resolved (or negative) host mapping stays cached. Domains change
/// rarely (add/verify/remove are admin actions), so this can be generous;
/// `invalidate` drops the entry immediately on those actions anyway.
pub const HOST_ROUTE_TTL_SECS: u64 = 300;

/// Normalizes a `Host` header value for lookup: trimmed, lowercased, a
/// trailing `:port` stripped if present (IPv6 literals are not a concern
/// here, only DNS hosts), and a trailing `.` (a fully-qualified DNS name,
/// e.g. `go.acme.com.`) stripped too so it still matches the stored host
/// without the dot.
fn normalize_host(host: &str) -> String {
    let host = host.trim();
    let host = host.rsplit_once(':').map_or(host, |(h, _)| h);
    let host = host.strip_suffix('.').unwrap_or(host);
    host.to_lowercase()
}

pub struct HostRouter {
    store: Arc<dyn Store>,
    public_host: Option<String>,
    cache: Moka<String, Option<DomainRoute>>,
    invalidator: Option<Arc<Invalidator>>,
}

impl HostRouter {
    pub fn new(
        store: Arc<dyn Store>,
        public_host: Option<String>,
        invalidator: Option<Arc<Invalidator>>,
    ) -> Self {
        Self::with_ttl(store, public_host, invalidator, HOST_ROUTE_TTL_SECS)
    }

    fn with_ttl(
        store: Arc<dyn Store>,
        public_host: Option<String>,
        invalidator: Option<Arc<Invalidator>>,
        ttl_secs: u64,
    ) -> Self {
        let public_host = public_host.map(|h| normalize_host(&h));
        let cache = Moka::builder()
            .time_to_live(std::time::Duration::from_secs(ttl_secs))
            .build();
        HostRouter {
            store,
            public_host,
            cache,
            invalidator,
        }
    }

    /// Resolves a `Host` header to a route. `None` means the caller should
    /// treat the host as not found (unknown, or a domain not yet verified).
    pub async fn resolve(&self, host: &str) -> Option<DomainRoute> {
        let key = normalize_host(host);
        if let Some(public_host) = &self.public_host {
            if key == *public_host {
                return Some(DomainRoute {
                    domain_id: crate::domain::SHARED_DOMAIN_ID,
                    tenant_id: DEFAULT_TENANT,
                });
            }
        }
        if let Some(cached) = self.cache.get(&key) {
            return cached;
        }
        let route = self.lookup(&key).await;
        self.cache.insert(key, route.clone());
        route
    }

    /// Looks up the store directly, translating a `Verified` domain into a
    /// route and anything else (absent, `Pending`) into `None`. Store errors
    /// are treated as "not found" rather than propagated: a lookup failure on
    /// the redirect hot path must not turn into a 500, it should 404 and let
    /// the next request retry (the entry is not cached on error, since it was
    /// not inserted by `resolve` in that branch's caller... see note below).
    async fn lookup(&self, host: &str) -> Option<DomainRoute> {
        match self.store.get_domain_by_host(host).await {
            Ok(Some(domain)) if domain.status == DomainStatus::Verified => Some(DomainRoute {
                domain_id: domain.id,
                tenant_id: domain.tenant_id,
            }),
            Ok(_) => None,
            Err(_) => None,
        }
    }

    /// Drops the cached entry for `host`. Called on domain add/remove/verify
    /// (Task 6) so the change is visible immediately on this replica. Cross-
    /// replica invalidation is not wired yet (the `Invalidator` pub/sub
    /// channel only carries `link:<id>` messages today); the TTL bounds
    /// staleness on other replicas in the meantime. Wiring a `domain:<host>`
    /// message through the same channel is a natural follow-up once Task 6
    /// lands.
    pub async fn invalidate(&self, host: &str) {
        let key = normalize_host(host);
        self.cache.invalidate(&key);
    }

    /// Whether this router has a cross-replica invalidator configured. Exposed
    /// for callers that want to know if `invalidate` is local-only; not used
    /// on the hot path.
    pub fn has_invalidator(&self) -> bool {
        self.invalidator.is_some()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::Domain;
    use crate::store::{LinkHealth, OutboxDelivery, OutboxRow, Record, StoreError};
    use crate::tenant::{Membership, Tenant, TenantId, User};
    use crate::webhooks::WebhookSubscription;
    use async_trait::async_trait;
    use std::collections::HashMap;
    use std::sync::atomic::{AtomicU32, Ordering};

    /// Fake `Store` that only implements `get_domain_by_host` meaningfully;
    /// every other method is `unimplemented!()` since the router must never
    /// touch them (mirrors the pattern in `webhooks::delivery::tests`).
    struct FakeStore {
        domains: HashMap<String, Domain>,
        calls: AtomicU32,
    }

    impl FakeStore {
        fn new(domains: Vec<Domain>) -> Self {
            let domains = domains.into_iter().map(|d| (d.host.clone(), d)).collect();
            FakeStore {
                domains,
                calls: AtomicU32::new(0),
            }
        }
        fn calls(&self) -> u32 {
            self.calls.load(Ordering::SeqCst)
        }
    }

    #[async_trait]
    impl Store for FakeStore {
        async fn next_id(&self, _tenant: TenantId) -> Result<u64, StoreError> {
            unimplemented!()
        }
        async fn get_link(
            &self,
            _tenant: TenantId,
            _id: u64,
        ) -> Result<Option<Record>, StoreError> {
            unimplemented!()
        }
        async fn put_link(
            &self,
            _tenant: TenantId,
            _id: u64,
            _rec: &Record,
        ) -> Result<(), StoreError> {
            unimplemented!()
        }
        async fn get_alias(
            &self,
            _domain_id: u64,
            _alias: &str,
        ) -> Result<Option<u64>, StoreError> {
            unimplemented!()
        }
        async fn put_alias_and_link(
            &self,
            _tenant: TenantId,
            _domain_id: u64,
            _alias: &str,
            _id: u64,
            _rec: &Record,
        ) -> Result<bool, StoreError> {
            unimplemented!()
        }
        async fn put_link_tx(
            &self,
            _tenant: TenantId,
            _id: u64,
            _rec: &Record,
            _deliveries: &[OutboxRow],
        ) -> Result<(), StoreError> {
            unimplemented!()
        }
        async fn put_alias_and_link_tx(
            &self,
            _tenant: TenantId,
            _domain_id: u64,
            _alias: &str,
            _id: u64,
            _rec: &Record,
            _deliveries: &[OutboxRow],
        ) -> Result<bool, StoreError> {
            unimplemented!()
        }
        async fn delete_link_tx(
            &self,
            _tenant: TenantId,
            _id: u64,
            _deliveries: &[OutboxRow],
        ) -> Result<(), StoreError> {
            unimplemented!()
        }
        async fn list_links(
            &self,
            _tenant: TenantId,
            _after: Option<u64>,
            _limit: usize,
            _tag: Option<&str>,
            _folder: Option<&str>,
        ) -> Result<Vec<(u64, Record)>, StoreError> {
            unimplemented!()
        }
        async fn search_links(
            &self,
            _tenant: TenantId,
            _q: &str,
            _after: Option<u64>,
            _limit: usize,
            _tag: Option<&str>,
            _folder: Option<&str>,
        ) -> Result<Vec<(u64, Record)>, StoreError> {
            unimplemented!()
        }
        async fn list_tags(&self, _tenant: TenantId) -> Result<Vec<(String, u64)>, StoreError> {
            unimplemented!()
        }
        async fn list_folders(&self, _tenant: TenantId) -> Result<Vec<(String, u64)>, StoreError> {
            unimplemented!()
        }
        async fn list_aliases(&self, _tenant: TenantId) -> Result<Vec<(String, u64)>, StoreError> {
            unimplemented!()
        }
        async fn delete_link(&self, _tenant: TenantId, _id: u64) -> Result<(), StoreError> {
            unimplemented!()
        }
        async fn delete_alias(&self, _tenant: TenantId, _alias: &str) -> Result<(), StoreError> {
            unimplemented!()
        }
        async fn list_webhooks(
            &self,
            _tenant: TenantId,
        ) -> Result<Vec<WebhookSubscription>, StoreError> {
            unimplemented!()
        }
        async fn get_webhook(
            &self,
            _tenant: TenantId,
            _id: u64,
        ) -> Result<Option<WebhookSubscription>, StoreError> {
            unimplemented!()
        }
        async fn put_webhook(
            &self,
            _tenant: TenantId,
            _sub: &WebhookSubscription,
        ) -> Result<(), StoreError> {
            unimplemented!()
        }
        async fn delete_webhook(&self, _tenant: TenantId, _id: u64) -> Result<bool, StoreError> {
            unimplemented!()
        }
        async fn next_webhook_id(&self, _tenant: TenantId) -> Result<u64, StoreError> {
            unimplemented!()
        }
        async fn list_api_tokens(
            &self,
            _tenant: TenantId,
        ) -> Result<Vec<crate::auth::ApiToken>, StoreError> {
            unimplemented!()
        }
        async fn get_api_token_by_hash(
            &self,
            _hash: &str,
        ) -> Result<Option<crate::auth::ApiToken>, StoreError> {
            unimplemented!()
        }
        async fn put_api_token(
            &self,
            _tenant: TenantId,
            _token: &crate::auth::ApiToken,
        ) -> Result<(), StoreError> {
            unimplemented!()
        }
        async fn delete_api_token(&self, _tenant: TenantId, _id: u64) -> Result<bool, StoreError> {
            unimplemented!()
        }
        async fn next_api_token_id(&self, _tenant: TenantId) -> Result<u64, StoreError> {
            unimplemented!()
        }
        async fn put_session(
            &self,
            _tenant: TenantId,
            _session: &crate::auth::Session,
        ) -> Result<(), StoreError> {
            unimplemented!()
        }
        async fn get_session_by_hash(
            &self,
            _token_hash: &str,
            _now: u64,
        ) -> Result<Option<crate::auth::Session>, StoreError> {
            unimplemented!()
        }
        async fn delete_session(&self, _token_hash: &str) -> Result<(), StoreError> {
            unimplemented!()
        }
        async fn gc_sessions(&self, _now: u64) -> Result<(), StoreError> {
            unimplemented!()
        }
        async fn bump_visits(&self, _tenant: TenantId, _id: u64) -> Result<u64, StoreError> {
            unimplemented!()
        }
        async fn visits(&self, _tenant: TenantId, _id: u64) -> Result<u64, StoreError> {
            unimplemented!()
        }
        async fn put_link_health(
            &self,
            _tenant: TenantId,
            _id: u64,
            _health: &LinkHealth,
        ) -> Result<(), StoreError> {
            unimplemented!()
        }
        async fn list_link_health(
            &self,
            _tenant: TenantId,
        ) -> Result<Vec<(u64, LinkHealth)>, StoreError> {
            unimplemented!()
        }
        async fn link_health_for(
            &self,
            _tenant: TenantId,
            _ids: &[u64],
        ) -> Result<Vec<(u64, LinkHealth)>, StoreError> {
            unimplemented!()
        }
        async fn list_broken_link_ids(&self, _tenant: TenantId) -> Result<Vec<u64>, StoreError> {
            unimplemented!()
        }
        async fn try_acquire_health_lease(
            &self,
            _holder: &str,
            _ttl_secs: u64,
        ) -> Result<bool, StoreError> {
            unimplemented!()
        }
        async fn put_sheets_connection(
            &self,
            _tenant: TenantId,
            _c: &crate::sheets::SheetsConnection,
        ) -> Result<(), StoreError> {
            unimplemented!()
        }
        async fn get_sheets_connection(
            &self,
            _tenant: TenantId,
        ) -> Result<Option<crate::sheets::SheetsConnection>, StoreError> {
            unimplemented!()
        }
        async fn delete_sheets_connection(&self, _tenant: TenantId) -> Result<(), StoreError> {
            unimplemented!()
        }
        async fn try_acquire_sheets_lease(
            &self,
            _holder: &str,
            _ttl_secs: u64,
        ) -> Result<bool, StoreError> {
            unimplemented!()
        }
        async fn next_pixel_id(&self, _tenant: TenantId) -> Result<u64, StoreError> {
            unimplemented!()
        }
        async fn get_pixel(
            &self,
            _tenant: TenantId,
            _id: u64,
        ) -> Result<Option<crate::pixel::PixelConfig>, StoreError> {
            unimplemented!()
        }
        async fn put_pixel(
            &self,
            _tenant: TenantId,
            _config: &crate::pixel::PixelConfig,
        ) -> Result<(), StoreError> {
            unimplemented!()
        }
        async fn delete_pixel(&self, _tenant: TenantId, _id: u64) -> Result<bool, StoreError> {
            unimplemented!()
        }
        async fn list_pixels(
            &self,
            _tenant: TenantId,
        ) -> Result<Vec<crate::pixel::PixelConfig>, StoreError> {
            unimplemented!()
        }
        async fn get_wellknown(
            &self,
            _tenant: TenantId,
            _name: &str,
        ) -> Result<Option<String>, StoreError> {
            unimplemented!()
        }
        async fn put_wellknown(
            &self,
            _tenant: TenantId,
            _name: &str,
            _body: &str,
        ) -> Result<(), StoreError> {
            unimplemented!()
        }
        async fn delete_wellknown(&self, _tenant: TenantId, _name: &str) -> Result<(), StoreError> {
            unimplemented!()
        }
        async fn put_tenant(&self, _t: &Tenant) -> Result<(), StoreError> {
            unimplemented!()
        }
        async fn get_tenant(&self, _id: TenantId) -> Result<Option<Tenant>, StoreError> {
            unimplemented!()
        }
        async fn list_tenants(&self) -> Result<Vec<Tenant>, StoreError> {
            unimplemented!()
        }
        async fn next_user_id(&self) -> Result<u64, StoreError> {
            unimplemented!()
        }
        async fn next_tenant_id(&self) -> Result<u64, StoreError> {
            unimplemented!()
        }
        async fn put_user(&self, _u: &User) -> Result<(), StoreError> {
            unimplemented!()
        }
        async fn get_user_by_subject(&self, _subject: &str) -> Result<Option<User>, StoreError> {
            unimplemented!()
        }
        async fn get_user_by_id(&self, _id: u64) -> Result<Option<User>, StoreError> {
            unimplemented!()
        }
        async fn put_membership(&self, _m: &Membership) -> Result<(), StoreError> {
            unimplemented!()
        }
        async fn get_membership(
            &self,
            _user_id: u64,
            _tenant: TenantId,
        ) -> Result<Option<Membership>, StoreError> {
            unimplemented!()
        }
        async fn list_memberships_for_user(
            &self,
            _user_id: u64,
        ) -> Result<Vec<Membership>, StoreError> {
            unimplemented!()
        }
        async fn next_domain_id(&self) -> Result<u64, StoreError> {
            unimplemented!()
        }
        async fn get_domain_by_host(&self, host: &str) -> Result<Option<Domain>, StoreError> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            Ok(self.domains.get(host).cloned())
        }
        async fn get_domain(
            &self,
            _tenant: TenantId,
            _id: u64,
        ) -> Result<Option<Domain>, StoreError> {
            unimplemented!()
        }
        async fn list_domains(&self, _tenant: TenantId) -> Result<Vec<Domain>, StoreError> {
            unimplemented!()
        }
        async fn put_domain(&self, _domain: &Domain) -> Result<(), StoreError> {
            unimplemented!()
        }
        async fn set_domain_status(
            &self,
            _tenant: TenantId,
            _id: u64,
            _status: DomainStatus,
            _verified_at: Option<u64>,
        ) -> Result<(), StoreError> {
            unimplemented!()
        }
        async fn delete_domain(&self, _tenant: TenantId, _id: u64) -> Result<(), StoreError> {
            unimplemented!()
        }
        async fn next_invite_id(&self) -> Result<u64, StoreError> {
            unimplemented!()
        }
        async fn create_invite(&self, _inv: &crate::invite::Invite) -> Result<(), StoreError> {
            unimplemented!()
        }
        async fn get_invite_by_hash(
            &self,
            _token_hash: &str,
            _now: u64,
        ) -> Result<Option<crate::invite::Invite>, StoreError> {
            unimplemented!()
        }
        async fn mark_invite_accepted(
            &self,
            _id: u64,
            _accepted_by: u64,
            _now: u64,
        ) -> Result<bool, StoreError> {
            unimplemented!()
        }
        async fn list_invites(
            &self,
            _tenant: TenantId,
        ) -> Result<Vec<crate::invite::Invite>, StoreError> {
            unimplemented!()
        }
        async fn delete_invite(&self, _tenant: TenantId, _id: u64) -> Result<(), StoreError> {
            unimplemented!()
        }
        async fn enqueue_deliveries(&self, _rows: &[OutboxRow]) -> Result<(), StoreError> {
            unimplemented!()
        }
        async fn claim_due_deliveries(
            &self,
            _now: u64,
            _limit: i64,
        ) -> Result<Vec<OutboxDelivery>, StoreError> {
            unimplemented!()
        }
        async fn mark_delivered(&self, _id: i64) -> Result<(), StoreError> {
            unimplemented!()
        }
        async fn mark_retry(
            &self,
            _id: i64,
            _next_attempt_at: u64,
            _attempts: u32,
        ) -> Result<(), StoreError> {
            unimplemented!()
        }
        async fn mark_dead(&self, _id: i64, _attempts: u32) -> Result<(), StoreError> {
            unimplemented!()
        }
    }

    fn domain(id: u64, tenant: u64, host: &str, status: DomainStatus) -> Domain {
        Domain {
            id,
            tenant_id: TenantId(tenant),
            host: host.into(),
            token: "tok".into(),
            status,
            created: 0,
            verified_at: None,
        }
    }

    #[tokio::test]
    async fn verified_domain_resolves_to_its_route() {
        let store = Arc::new(FakeStore::new(vec![domain(
            7,
            3,
            "go.acme.com",
            DomainStatus::Verified,
        )]));
        let router = HostRouter::new(store, None, None);
        let route = router.resolve("go.acme.com").await;
        assert_eq!(
            route,
            Some(DomainRoute {
                domain_id: 7,
                tenant_id: TenantId(3)
            })
        );
    }

    #[tokio::test]
    async fn pending_domain_resolves_to_none() {
        let store = Arc::new(FakeStore::new(vec![domain(
            8,
            3,
            "go.acme.com",
            DomainStatus::Pending,
        )]));
        let router = HostRouter::new(store, None, None);
        assert_eq!(router.resolve("go.acme.com").await, None);
    }

    #[tokio::test]
    async fn unknown_host_resolves_to_none() {
        let store = Arc::new(FakeStore::new(vec![]));
        let router = HostRouter::new(store, None, None);
        assert_eq!(router.resolve("nope.example.com").await, None);
    }

    #[tokio::test]
    async fn public_host_resolves_to_shared_route() {
        let store = Arc::new(FakeStore::new(vec![]));
        let router = HostRouter::new(store, Some("quark.example.com".into()), None);
        assert_eq!(
            router.resolve("quark.example.com").await,
            Some(DomainRoute {
                domain_id: crate::domain::SHARED_DOMAIN_ID,
                tenant_id: DEFAULT_TENANT,
            })
        );
    }

    #[tokio::test]
    async fn public_host_match_is_case_and_port_insensitive() {
        let store = Arc::new(FakeStore::new(vec![]));
        let router = HostRouter::new(store, Some("Quark.Example.com".into()), None);
        assert_eq!(
            router.resolve("QUARK.EXAMPLE.COM:443").await,
            Some(DomainRoute {
                domain_id: crate::domain::SHARED_DOMAIN_ID,
                tenant_id: DEFAULT_TENANT,
            })
        );
    }

    /// P3 Task 4 (review Minor from Task 3 folded in here): a `Host` header
    /// with surrounding whitespace or a trailing FQDN dot must still match
    /// the stored host.
    #[tokio::test]
    async fn public_host_match_trims_and_strips_trailing_dot() {
        let store = Arc::new(FakeStore::new(vec![]));
        let router = HostRouter::new(store, Some("quark.example.com".into()), None);
        assert_eq!(
            router.resolve(" quark.example.com. ").await,
            Some(DomainRoute {
                domain_id: crate::domain::SHARED_DOMAIN_ID,
                tenant_id: DEFAULT_TENANT,
            })
        );
    }

    #[tokio::test]
    async fn second_resolve_hits_l1_cache_store_called_once() {
        let store = Arc::new(FakeStore::new(vec![domain(
            7,
            3,
            "go.acme.com",
            DomainStatus::Verified,
        )]));
        let router = HostRouter::new(store.clone(), None, None);
        let _ = router.resolve("go.acme.com").await;
        let _ = router.resolve("go.acme.com").await;
        assert_eq!(store.calls(), 1);
    }

    #[tokio::test]
    async fn negative_lookup_is_cached_too() {
        let store = Arc::new(FakeStore::new(vec![]));
        let router = HostRouter::new(store.clone(), None, None);
        let _ = router.resolve("nope.example.com").await;
        let _ = router.resolve("nope.example.com").await;
        assert_eq!(store.calls(), 1);
    }

    #[tokio::test]
    async fn invalidate_drops_cache_entry_so_next_resolve_hits_store_again() {
        let store = Arc::new(FakeStore::new(vec![domain(
            7,
            3,
            "go.acme.com",
            DomainStatus::Verified,
        )]));
        let router = HostRouter::new(store.clone(), None, None);
        let _ = router.resolve("go.acme.com").await;
        assert_eq!(store.calls(), 1);
        router.invalidate("go.acme.com").await;
        let _ = router.resolve("go.acme.com").await;
        assert_eq!(store.calls(), 2);
    }
}
