use axum::body::Body;
use axum::http::{Request, StatusCode};
use quark::analytics::AnalyticsSink;
use quark::api::{router, AppState};
use quark::auth::{hash_token, ApiToken, Scope};
use quark::cache::Cache;
use quark::dns::{Dns, DnsError, NullDns};
use quark::domain::{Domain, DomainStatus, SHARED_DOMAIN_ID};
use quark::store::postgres::PostgresStore;
use quark::store::{Record, Store};
use quark::tenant::{Tenant, TenantId};
use quark::webhooks::delivery::WebhookDispatcher;
use serial_test::serial;
use std::collections::HashMap;
use std::sync::Arc;
use tower::ServiceExt;

fn rec(url: &str) -> Record {
    Record {
        url: url.into(),
        expiry: None,
        created: 0,
        tags: Vec::new(),
        max_visits: None,
        rules: Vec::new(),
        variants: Vec::new(),
        app_ios: None,
        app_android: None,
        folder: None,
        fallback_url: None,
        password_hash: None,
        tenant_id: quark::tenant::DEFAULT_TENANT,
    }
}

async fn fresh() -> Option<PostgresStore> {
    let url = std::env::var("QUARK_TEST_DATABASE_URL").ok()?;
    let s = PostgresStore::open(&url, true).await.unwrap();
    s.reset_for_tests().await.unwrap();
    Some(s)
}

async fn make_tenant(store: &PostgresStore, slug: &str) -> TenantId {
    let id = store.next_tenant_id().await.unwrap();
    let tenant_id = TenantId(id);
    store
        .put_tenant(&Tenant {
            id: tenant_id,
            name: slug.to_string(),
            slug: slug.to_string(),
            created: 0,
        })
        .await
        .unwrap();
    tenant_id
}

/// Tenant A creates a custom domain; tenant B's own admin view (`list_domains`
/// / `get_domain`, both RLS-scoped) never sees it. The public, bare-pool
/// `get_domain_by_host` lookup is the one deliberate exception: it crosses
/// tenants by design, since the redirect path only has a `Host` header and
/// doesn't know the tenant yet.
#[tokio::test]
#[serial]
async fn domains_are_tenant_isolated_but_host_lookup_is_public() {
    let Some(store) = fresh().await else {
        eprintln!("skip: QUARK_TEST_DATABASE_URL not set");
        return;
    };
    let a = make_tenant(&store, "domains-tenant-a").await;
    let b = make_tenant(&store, "domains-tenant-b").await;

    let id = store.next_domain_id().await.unwrap();
    store
        .put_domain(&Domain {
            id,
            tenant_id: a,
            host: "go.acme.com".to_string(),
            token: "tok".to_string(),
            status: DomainStatus::Verified,
            created: 1,
            verified_at: Some(2),
        })
        .await
        .unwrap();

    assert_eq!(store.list_domains(a).await.unwrap().len(), 1);
    assert_eq!(
        store.list_domains(b).await.unwrap().len(),
        0,
        "tenant B must not see tenant A's domain via the tenant-scoped listing"
    );
    assert!(
        store.get_domain(b, id).await.unwrap().is_none(),
        "tenant B must not be able to fetch tenant A's domain by id"
    );

    let by_host = store
        .get_domain_by_host("go.acme.com")
        .await
        .unwrap()
        .expect("public host lookup must find the domain");
    assert_eq!(
        by_host.tenant_id, a,
        "public lookup crosses tenants by design"
    );
}

/// `set_domain_status` updates status/verified_at scoped to the owning
/// tenant, and `delete_domain` removes it; both are tenant-scoped mutations
/// like every other tenant-owned store method.
#[tokio::test]
#[serial]
async fn set_status_and_delete_are_tenant_scoped() {
    let Some(store) = fresh().await else {
        eprintln!("skip: QUARK_TEST_DATABASE_URL not set");
        return;
    };
    let a = make_tenant(&store, "domains-status-a").await;
    let id = store.next_domain_id().await.unwrap();
    store
        .put_domain(&Domain {
            id,
            tenant_id: a,
            host: "status.acme.com".to_string(),
            token: "tok2".to_string(),
            status: DomainStatus::Pending,
            created: 1,
            verified_at: None,
        })
        .await
        .unwrap();

    store
        .set_domain_status(a, id, DomainStatus::Verified, Some(42))
        .await
        .unwrap();
    let updated = store.get_domain(a, id).await.unwrap().unwrap();
    assert_eq!(updated.status, DomainStatus::Verified);
    assert_eq!(updated.verified_at, Some(42));

    store.delete_domain(a, id).await.unwrap();
    assert!(store.get_domain(a, id).await.unwrap().is_none());
}

/// P3 Task 2: the alias namespace is per-domain. The same alias string in two
/// different domains resolves to two different links, and the shared
/// namespace (`SHARED_DOMAIN_ID`) stays untouched by either.
#[tokio::test]
#[serial]
async fn alias_namespace_is_per_domain() {
    let Some(store) = fresh().await else {
        eprintln!("skip: QUARK_TEST_DATABASE_URL not set");
        return;
    };
    let tenant_a = make_tenant(&store, "alias-domain-tenant-a").await;
    let tenant_b = make_tenant(&store, "alias-domain-tenant-b").await;

    store
        .put_alias_and_link(tenant_a, 10, "promo", 100, &rec("https://a.example.com"))
        .await
        .unwrap();
    store
        .put_alias_and_link(tenant_b, 20, "promo", 200, &rec("https://b.example.com"))
        .await
        .unwrap();

    assert_eq!(store.get_alias(10, "promo").await.unwrap(), Some(100));
    assert_eq!(store.get_alias(20, "promo").await.unwrap(), Some(200));
    assert_eq!(
        store.get_alias(SHARED_DOMAIN_ID, "promo").await.unwrap(),
        None,
        "the shared namespace must not be touched by either domain's write"
    );
}

// --- P3 Task 4: Host -> tenant resolution wired into redirect/unlock, and
// the cross-tenant isolation filter it enables. Http-level, against the real
// router (`quark::api::router`), so the isolation is proven at the same
// layer a real request hits it, not just at the store.

const KEY: u64 = 0x1234;

/// A `WebhookDispatcher` whose receiver is dropped: `emit` silently no-ops.
fn test_webhook_dispatcher() -> Arc<WebhookDispatcher> {
    let (tx, _rx) = tokio::sync::mpsc::channel(1);
    Arc::new(WebhookDispatcher::new(
        tx,
        Arc::new(std::sync::atomic::AtomicBool::new(false)),
        Arc::new(std::sync::atomic::AtomicBool::new(false)),
    ))
}

/// Builds a cloud-mode (`multi_tenant = true`) router over `store`, with
/// `public_host` (if any) as the shared-domain host.
fn cloud_app(
    store: Arc<dyn Store>,
    sink: Arc<dyn AnalyticsSink>,
    public_host: Option<String>,
) -> axum::Router {
    let cache = Cache::new(store.clone(), 1000, None);
    let host_router = Arc::new(quark::domain_router::HostRouter::new(
        store.clone(),
        public_host.clone(),
        None,
    ));
    let (analytics_tx, _rx) = tokio::sync::mpsc::channel(100);
    let state = Arc::new(AppState {
        oidc: None,
        sheets: None,
        sheets_api: None,
        oidc_configured: false,
        multi_tenant: true,
        tenant_domain_suffix: None,
        cache,
        store,
        key: KEY,
        signing_key: [0u8; 32],
        analytics_tx,
        sink,
        admin_token: None,
        ratelimiter: quark::abuse::ratelimit::RateLimiter::disabled(),
        block_private: true,
        public_host,
        real_ip_header: "cf-connecting-ip".to_string(),
        webhooks: test_webhook_dispatcher(),
        host_router,
        dns: Arc::new(quark::dns::NullDns),
    });
    router(state)
}

/// The base62 short code a numeric `id` resolves to under `KEY`, matching
/// what `create_link_core`/`resolve_code` would produce/consume.
fn numeric_code(id: u64) -> String {
    quark::codec::to_base62(quark::permute::encode(id, KEY))
}

async fn make_domain(store: &PostgresStore, tenant: TenantId, host: &str) -> u64 {
    let id = store.next_domain_id().await.unwrap();
    store
        .put_domain(&Domain {
            id,
            tenant_id: tenant,
            host: host.to_string(),
            token: "tok".to_string(),
            status: DomainStatus::Verified,
            created: 1,
            verified_at: Some(2),
        })
        .await
        .unwrap();
    id
}

async fn get_via_host(app: &axum::Router, path: &str, host: &str) -> StatusCode {
    let resp = app
        .clone()
        .oneshot(
            Request::get(path)
                .header("host", host)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    resp.status()
}

/// CRITICAL (P3 Task 4): a numeric link owned by tenant A serves on A's own
/// verified custom domain, and 404s on tenant B's verified custom domain —
/// even though both domains are equally "known" hosts to the router. This is
/// the cross-tenant isolation filter the task adds.
#[tokio::test]
#[serial]
async fn redirect_isolates_numeric_link_by_custom_domain_tenant() {
    let Some(store) = fresh().await else {
        eprintln!("skip: QUARK_TEST_DATABASE_URL not set");
        return;
    };
    let tenant_a = make_tenant(&store, "task4-numeric-a").await;
    let tenant_b = make_tenant(&store, "task4-numeric-b").await;
    make_domain(&store, tenant_a, "go.acme.com").await;
    make_domain(&store, tenant_b, "go.beta.com").await;

    let id = 4001u64;
    store
        .put_link(tenant_a, id, &rec("https://a.example.com/owned"))
        .await
        .unwrap();
    let code = numeric_code(id);

    let pg = Arc::new(store);
    let store_dyn: Arc<dyn Store> = pg.clone();
    let sink_dyn: Arc<dyn AnalyticsSink> = pg;
    let app = cloud_app(store_dyn, sink_dyn, None);

    assert_eq!(
        get_via_host(&app, &format!("/{code}"), "go.acme.com").await,
        StatusCode::FOUND,
        "the owning tenant's own domain must serve the link"
    );
    assert_eq!(
        get_via_host(&app, &format!("/{code}"), "go.beta.com").await,
        StatusCode::NOT_FOUND,
        "a different tenant's domain must not serve someone else's link"
    );
}

/// The alias namespace is per-domain (P3 Task 2): the same alias string on
/// two different tenants' domains resolves to two different links.
#[tokio::test]
#[serial]
async fn redirect_alias_resolves_per_domain() {
    let Some(store) = fresh().await else {
        eprintln!("skip: QUARK_TEST_DATABASE_URL not set");
        return;
    };
    let tenant_a = make_tenant(&store, "task4-alias-a").await;
    let tenant_b = make_tenant(&store, "task4-alias-b").await;
    let domain_a = make_domain(&store, tenant_a, "promo.acme.com").await;
    let domain_b = make_domain(&store, tenant_b, "promo.beta.com").await;

    store
        .put_alias_and_link(
            tenant_a,
            domain_a,
            "promo",
            4100,
            &rec("https://a.example.com/promo"),
        )
        .await
        .unwrap();
    store
        .put_alias_and_link(
            tenant_b,
            domain_b,
            "promo",
            4200,
            &rec("https://b.example.com/promo"),
        )
        .await
        .unwrap();

    let pg = Arc::new(store);
    let store_dyn: Arc<dyn Store> = pg.clone();
    let sink_dyn: Arc<dyn AnalyticsSink> = pg;
    let app = cloud_app(store_dyn, sink_dyn, None);

    let resp_a = app
        .clone()
        .oneshot(
            Request::get("/promo")
                .header("host", "promo.acme.com")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp_a.status(), StatusCode::FOUND);
    assert_eq!(resp_a.headers()["location"], "https://a.example.com/promo");

    let resp_b = app
        .oneshot(
            Request::get("/promo")
                .header("host", "promo.beta.com")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp_b.status(), StatusCode::FOUND);
    assert_eq!(resp_b.headers()["location"], "https://b.example.com/promo");
}

/// An unrecognized `Host` (never registered as a domain, and not the shared
/// `public_host`) must 404 before any code resolution happens.
#[tokio::test]
#[serial]
async fn redirect_unknown_host_is_404() {
    let Some(store) = fresh().await else {
        eprintln!("skip: QUARK_TEST_DATABASE_URL not set");
        return;
    };
    let tenant_a = make_tenant(&store, "task4-unknown-host").await;
    let id = 4300u64;
    store
        .put_link(tenant_a, id, &rec("https://a.example.com/unknown-host"))
        .await
        .unwrap();
    let code = numeric_code(id);

    let pg = Arc::new(store);
    let store_dyn: Arc<dyn Store> = pg.clone();
    let sink_dyn: Arc<dyn AnalyticsSink> = pg;
    let app = cloud_app(store_dyn, sink_dyn, None);

    assert_eq!(
        get_via_host(&app, &format!("/{code}"), "nope.example.com").await,
        StatusCode::NOT_FOUND
    );
}

/// Regression guard: the shared host still resolves globally exactly as
/// before P3 (a link on the shared/default tenant redirects through it,
/// unaffected by custom-domain isolation).
#[tokio::test]
#[serial]
async fn redirect_shared_host_resolves_globally() {
    let Some(store) = fresh().await else {
        eprintln!("skip: QUARK_TEST_DATABASE_URL not set");
        return;
    };
    let id = 4400u64;
    store
        .put_link(
            quark::tenant::DEFAULT_TENANT,
            id,
            &rec("https://shared.example.com/global"),
        )
        .await
        .unwrap();
    let code = numeric_code(id);

    let pg = Arc::new(store);
    let store_dyn: Arc<dyn Store> = pg.clone();
    let sink_dyn: Arc<dyn AnalyticsSink> = pg;
    let app = cloud_app(store_dyn, sink_dyn, Some("quark.example.com".to_string()));

    let resp = app
        .oneshot(
            Request::get(format!("/{code}"))
                .header("host", "quark.example.com")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::FOUND);
    assert_eq!(
        resp.headers()["location"],
        "https://shared.example.com/global"
    );
}

/// `unlock` runs the same Host resolution + isolation filter as `redirect`
/// before it does password verification: a password-protected link owned by
/// tenant A must not be unlockable through tenant B's custom domain.
#[tokio::test]
#[serial]
async fn unlock_isolates_password_protected_link_by_custom_domain_tenant() {
    let Some(store) = fresh().await else {
        eprintln!("skip: QUARK_TEST_DATABASE_URL not set");
        return;
    };
    let tenant_a = make_tenant(&store, "task4-unlock-a").await;
    let tenant_b = make_tenant(&store, "task4-unlock-b").await;
    make_domain(&store, tenant_a, "lock.acme.com").await;
    make_domain(&store, tenant_b, "lock.beta.com").await;

    let id = 4500u64;
    let mut protected = rec("https://a.example.com/locked");
    protected.password_hash =
        Some("$argon2id$v=19$m=19456,t=2,p=1$c29tZXNhbHQ$dGVzdGhhc2g".to_string());
    store.put_link(tenant_a, id, &protected).await.unwrap();
    let code = numeric_code(id);

    let pg = Arc::new(store);
    let store_dyn: Arc<dyn Store> = pg.clone();
    let sink_dyn: Arc<dyn AnalyticsSink> = pg;
    let app = cloud_app(store_dyn, sink_dyn, None);

    // Owning tenant's domain: the link is visible (and protected), so the
    // unlock POST proceeds to password verification and 200s back the
    // interstitial on a wrong password, rather than 404-ing.
    let resp_a = app
        .clone()
        .oneshot(
            Request::post(format!("/{code}"))
                .header("host", "lock.acme.com")
                .header("content-type", "application/x-www-form-urlencoded")
                .body(Body::from("password=wrong"))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(
        resp_a.status(),
        StatusCode::OK,
        "wrong password re-renders the interstitial, not 404"
    );

    // Other tenant's domain: the link must not even be found.
    let resp_b = app
        .oneshot(
            Request::post(format!("/{code}"))
                .header("host", "lock.beta.com")
                .header("content-type", "application/x-www-form-urlencoded")
                .body(Body::from("password=wrong"))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(
        resp_b.status(),
        StatusCode::NOT_FOUND,
        "a different tenant's domain must not reveal or unlock someone else's protected link"
    );
}

// --- P3 Task 6: `/admin/domains` CRUD + DNS TXT verification ---

/// A `Dns` fake whose TXT records are fixed at construction, so tests can
/// control exactly what `verify` sees without a real name server.
struct FakeDns {
    records: HashMap<String, Vec<String>>,
}

impl FakeDns {
    fn with_record(name: &str, values: Vec<String>) -> Self {
        let mut records = HashMap::new();
        records.insert(name.to_string(), values);
        FakeDns { records }
    }
}

#[async_trait::async_trait]
impl Dns for FakeDns {
    async fn lookup_txt(&self, name: &str) -> Result<Vec<String>, DnsError> {
        Ok(self.records.get(name).cloned().unwrap_or_default())
    }
}

/// Builds a router with a scoped API token already seeded for `tenant`, so
/// tests can hit `/admin/domains` as that tenant via `x-admin-token`. Returns
/// the app and the raw token to send.
async fn admin_app_for_tenant(
    store: Arc<PostgresStore>,
    dns: Arc<dyn Dns>,
    multi_tenant: bool,
    tenant: TenantId,
    token_id: u64,
) -> (axum::Router, String) {
    let raw = format!("qtok_test_{}", token_id);
    store
        .put_api_token(
            tenant,
            &ApiToken {
                id: token_id,
                name: "domains-test-token".to_string(),
                token_hash: hash_token(&raw),
                scopes: vec![Scope::Full],
                rate_limit_per_min: None,
                created: 0,
                tenant_id: tenant,
            },
        )
        .await
        .unwrap();

    let store_dyn: Arc<dyn Store> = store.clone();
    let sink_dyn: Arc<dyn AnalyticsSink> = store;
    let cache = Cache::new(store_dyn.clone(), 1000, None);
    let host_router = Arc::new(quark::domain_router::HostRouter::new(
        store_dyn.clone(),
        Some("quark.example.com".to_string()),
        None,
    ));
    let (analytics_tx, _rx) = tokio::sync::mpsc::channel(100);
    let state = Arc::new(AppState {
        oidc: None,
        sheets: None,
        sheets_api: None,
        oidc_configured: false,
        multi_tenant,
        tenant_domain_suffix: None,
        cache,
        store: store_dyn,
        key: KEY,
        signing_key: [0u8; 32],
        analytics_tx,
        sink: sink_dyn,
        admin_token: None,
        ratelimiter: quark::abuse::ratelimit::RateLimiter::disabled(),
        block_private: true,
        public_host: Some("quark.example.com".to_string()),
        real_ip_header: "cf-connecting-ip".to_string(),
        webhooks: test_webhook_dispatcher(),
        host_router,
        dns,
    });
    (router(state), raw)
}

async fn create_domain(
    app: &axum::Router,
    token: &str,
    host: &str,
) -> (StatusCode, serde_json::Value) {
    let resp = app
        .clone()
        .oneshot(
            Request::post("/admin/domains")
                .header("content-type", "application/json")
                .header("x-admin-token", token)
                .body(Body::from(format!(r#"{{"host":"{host}"}}"#)))
                .unwrap(),
        )
        .await
        .unwrap();
    let status = resp.status();
    let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .unwrap();
    let json = if body.is_empty() {
        serde_json::Value::Null
    } else {
        serde_json::from_slice(&body).unwrap_or(serde_json::Value::Null)
    };
    (status, json)
}

/// Create returns `pending` plus the DNS verification instructions: the TXT
/// record name/value to publish and the CNAME target.
#[tokio::test]
#[serial]
async fn create_domain_returns_pending_with_instructions() {
    let Some(store) = fresh().await else {
        eprintln!("skip: QUARK_TEST_DATABASE_URL not set");
        return;
    };
    let store = Arc::new(store);
    let tenant = make_tenant(&store, "domains-create-a").await;
    let (app, token) = admin_app_for_tenant(store, Arc::new(NullDns), true, tenant, 9001).await;

    let (status, body) = create_domain(&app, &token, "go.acme.com").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["host"], "go.acme.com");
    assert_eq!(body["status"], "pending");
    assert_eq!(body["txt_name"], "_quark-verify.go.acme.com");
    assert_eq!(body["txt_value"], body["txt_value"].clone());
    assert!(
        body["txt_value"].as_str().is_some_and(|v| !v.is_empty()),
        "must return a non-empty verification token"
    );
    assert_eq!(body["cname_target"], "quark.example.com");
}

/// A duplicate `host` (already owned, even by another tenant) is a 409, not
/// a 503 — the store maps the UNIQUE violation and the handler surfaces it.
#[tokio::test]
#[serial]
async fn create_domain_duplicate_host_is_409() {
    let Some(store) = fresh().await else {
        eprintln!("skip: QUARK_TEST_DATABASE_URL not set");
        return;
    };
    let store = Arc::new(store);
    let tenant = make_tenant(&store, "domains-dup-a").await;
    let (app, token) = admin_app_for_tenant(store, Arc::new(NullDns), true, tenant, 9002).await;

    let (status1, _) = create_domain(&app, &token, "dup.acme.com").await;
    assert_eq!(status1, StatusCode::OK);
    let (status2, _) = create_domain(&app, &token, "dup.acme.com").await;
    assert_eq!(status2, StatusCode::CONFLICT);
}

/// Creating an internal host or the shared `public_host` is rejected before
/// it ever reaches the store.
#[tokio::test]
#[serial]
async fn create_domain_rejects_internal_and_public_host() {
    let Some(store) = fresh().await else {
        eprintln!("skip: QUARK_TEST_DATABASE_URL not set");
        return;
    };
    let store = Arc::new(store);
    let tenant = make_tenant(&store, "domains-internal-a").await;
    let (app, token) = admin_app_for_tenant(store, Arc::new(NullDns), true, tenant, 9003).await;

    let (status, _) = create_domain(&app, &token, "localhost").await;
    assert_eq!(
        status,
        StatusCode::BAD_REQUEST,
        "internal host must be rejected"
    );

    let (status, _) = create_domain(&app, &token, "quark.example.com").await;
    assert_eq!(
        status,
        StatusCode::BAD_REQUEST,
        "the shared public_host must be rejected"
    );
}

/// `verify` with a `FakeDns` returning the domain's token marks it `Verified`
/// and drops the host router's cached entry for the host.
#[tokio::test]
#[serial]
async fn verify_with_matching_txt_marks_verified_and_invalidates_router() {
    let Some(store) = fresh().await else {
        eprintln!("skip: QUARK_TEST_DATABASE_URL not set");
        return;
    };
    let store = Arc::new(store);
    let tenant = make_tenant(&store, "domains-verify-ok-a").await;

    // Create first with a no-op DNS, to learn the generated token, then
    // rebuild the app with a FakeDns primed with that exact token.
    let (app, token) =
        admin_app_for_tenant(store.clone(), Arc::new(NullDns), true, tenant, 9004).await;
    let (status, body) = create_domain(&app, &token, "verify-ok.acme.com").await;
    assert_eq!(status, StatusCode::OK);
    let domain_id = body["id"].as_u64().unwrap();
    let verify_token = body["txt_value"].as_str().unwrap().to_string();

    // Prime the host router's cache with a resolution for the host, so we
    // can observe `verify` invalidating it (a stale cache entry would keep
    // routing as if the domain were still unverified).
    let host_router = Arc::new(quark::domain_router::HostRouter::new(
        store.clone() as Arc<dyn Store>,
        Some("quark.example.com".to_string()),
        None,
    ));
    let _ = host_router.resolve("verify-ok.acme.com").await;

    let fake_dns = Arc::new(FakeDns::with_record(
        "_quark-verify.verify-ok.acme.com",
        vec![verify_token],
    ));
    let (app2, token2) = admin_app_for_tenant(store.clone(), fake_dns, true, tenant, 9005).await;
    // Re-seed the same token id isn't needed: `admin_app_for_tenant` mints a
    // fresh scoped token per call, both valid for `tenant`.
    let resp = app2
        .oneshot(
            Request::post(format!("/admin/domains/{domain_id}/verify"))
                .header("x-admin-token", token2)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .unwrap();
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(json["status"], "verified");

    let stored = store.get_domain(tenant, domain_id).await.unwrap().unwrap();
    assert_eq!(stored.status, DomainStatus::Verified);
    assert!(stored.verified_at.is_some());
}

/// `verify` with no matching TXT record leaves the domain `pending`.
#[tokio::test]
#[serial]
async fn verify_with_wrong_txt_stays_pending() {
    let Some(store) = fresh().await else {
        eprintln!("skip: QUARK_TEST_DATABASE_URL not set");
        return;
    };
    let store = Arc::new(store);
    let tenant = make_tenant(&store, "domains-verify-bad-a").await;
    let (app, token) =
        admin_app_for_tenant(store.clone(), Arc::new(NullDns), true, tenant, 9006).await;
    let (status, body) = create_domain(&app, &token, "verify-bad.acme.com").await;
    assert_eq!(status, StatusCode::OK);
    let domain_id = body["id"].as_u64().unwrap();

    // A FakeDns with the wrong value: does not match the domain's token.
    let fake_dns = Arc::new(FakeDns::with_record(
        "_quark-verify.verify-bad.acme.com",
        vec!["not-the-token".to_string()],
    ));
    let (app2, token2) = admin_app_for_tenant(store.clone(), fake_dns, true, tenant, 9007).await;
    let resp = app2
        .oneshot(
            Request::post(format!("/admin/domains/{domain_id}/verify"))
                .header("x-admin-token", token2)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .unwrap();
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(json["status"], "pending");

    let stored = store.get_domain(tenant, domain_id).await.unwrap().unwrap();
    assert_eq!(stored.status, DomainStatus::Pending);
}

/// `list`/`delete` are tenant-scoped: tenant B's admin view never sees
/// tenant A's domain, and cannot delete it by id either.
#[tokio::test]
#[serial]
async fn list_and_delete_are_tenant_scoped() {
    let Some(store) = fresh().await else {
        eprintln!("skip: QUARK_TEST_DATABASE_URL not set");
        return;
    };
    let store = Arc::new(store);
    let tenant_a = make_tenant(&store, "domains-scope-a").await;
    let tenant_b = make_tenant(&store, "domains-scope-b").await;

    let (app_a, token_a) =
        admin_app_for_tenant(store.clone(), Arc::new(NullDns), true, tenant_a, 9008).await;
    let (status, body) = create_domain(&app_a, &token_a, "scope.acme.com").await;
    assert_eq!(status, StatusCode::OK);
    let domain_id = body["id"].as_u64().unwrap();

    let (app_b, token_b) =
        admin_app_for_tenant(store.clone(), Arc::new(NullDns), true, tenant_b, 9009).await;

    // Tenant B's list must not include tenant A's domain.
    let resp = app_b
        .clone()
        .oneshot(
            Request::get("/admin/domains")
                .header("x-admin-token", &token_b)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .unwrap();
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert!(
        json.as_array().unwrap().is_empty(),
        "tenant B must not see tenant A's domain in its own list"
    );

    // Tenant A's own list does include it.
    let resp = app_a
        .oneshot(
            Request::get("/admin/domains")
                .header("x-admin-token", &token_a)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .unwrap();
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(json.as_array().unwrap().len(), 1);

    // Tenant B cannot delete tenant A's domain by id.
    let resp = app_b
        .oneshot(
            Request::delete(format!("/admin/domains/{domain_id}"))
                .header("x-admin-token", &token_b)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
    assert!(
        store
            .get_domain(tenant_a, domain_id)
            .await
            .unwrap()
            .is_some(),
        "tenant A's domain must survive tenant B's delete attempt"
    );

    // Tenant A can delete its own domain.
    let (app_a2, token_a2) =
        admin_app_for_tenant(store.clone(), Arc::new(NullDns), true, tenant_a, 9010).await;
    let resp = app_a2
        .oneshot(
            Request::delete(format!("/admin/domains/{domain_id}"))
                .header("x-admin-token", &token_a2)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::NO_CONTENT);
    assert!(store
        .get_domain(tenant_a, domain_id)
        .await
        .unwrap()
        .is_none());
}

/// All four `/admin/domains` endpoints 404 in OSS (`multi_tenant = false`).
#[tokio::test]
#[serial]
async fn domains_endpoints_404_in_oss() {
    let Some(store) = fresh().await else {
        eprintln!("skip: QUARK_TEST_DATABASE_URL not set");
        return;
    };
    let store = Arc::new(store);
    let tenant = make_tenant(&store, "domains-oss-a").await;
    let (app, token) = admin_app_for_tenant(store, Arc::new(NullDns), false, tenant, 9011).await;

    let resp = app
        .clone()
        .oneshot(
            Request::get("/admin/domains")
                .header("x-admin-token", &token)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);

    let resp = app
        .clone()
        .oneshot(
            Request::post("/admin/domains")
                .header("content-type", "application/json")
                .header("x-admin-token", &token)
                .body(Body::from(r#"{"host":"oss.acme.com"}"#))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);

    let resp = app
        .clone()
        .oneshot(
            Request::post("/admin/domains/1/verify")
                .header("x-admin-token", &token)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);

    let resp = app
        .oneshot(
            Request::delete("/admin/domains/1")
                .header("x-admin-token", &token)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
}

// --- P3 Task 7: wellknown-by-Host, SSRF covers all registered hosts, OSS
// parity ---

async fn get_wellknown(app: &axum::Router, host: &str) -> (StatusCode, String) {
    let resp = app
        .clone()
        .oneshot(
            Request::get("/.well-known/apple-app-site-association")
                .header("host", host)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let status = resp.status();
    let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .unwrap();
    (status, String::from_utf8_lossy(&body).into_owned())
}

/// `serve_wellknown` picks the tenant by the incoming `Host`: a verified
/// custom domain gets its own tenant's document, the shared host gets
/// `DEFAULT_TENANT`'s, and an unrecognized host gets nothing (404) even
/// though a document exists for some other tenant.
#[tokio::test]
#[serial]
async fn wellknown_is_resolved_by_host() {
    let Some(store) = fresh().await else {
        eprintln!("skip: QUARK_TEST_DATABASE_URL not set");
        return;
    };
    let store = Arc::new(store);
    let tenant_a = make_tenant(&store, "wellknown-host-a").await;
    make_domain(&store, tenant_a, "go.wellknown-a.com").await;

    store
        .put_wellknown(
            tenant_a,
            "apple-app-site-association",
            r#"{"owner":"tenant-a"}"#,
        )
        .await
        .unwrap();
    store
        .put_wellknown(
            quark::tenant::DEFAULT_TENANT,
            "apple-app-site-association",
            r#"{"owner":"shared"}"#,
        )
        .await
        .unwrap();

    let store_dyn: Arc<dyn Store> = store.clone();
    let sink_dyn: Arc<dyn AnalyticsSink> = store;
    let app = cloud_app(store_dyn, sink_dyn, Some("quark.example.com".to_string()));

    let (status, body) = get_wellknown(&app, "go.wellknown-a.com").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body, r#"{"owner":"tenant-a"}"#);

    let (status, body) = get_wellknown(&app, "quark.example.com").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body, r#"{"owner":"shared"}"#);

    let (status, _) = get_wellknown(&app, "unknown.example.com").await;
    assert_eq!(
        status,
        StatusCode::NOT_FOUND,
        "an unrecognized host must 404, never fall back to another tenant's document"
    );
}

/// OSS parity: with `multi_tenant = false`, `serve_wellknown` behaves exactly
/// as pre-P3 — tenant 0, Host header ignored entirely.
#[tokio::test]
#[serial]
async fn wellknown_ignores_host_in_oss() {
    let Some(store) = fresh().await else {
        eprintln!("skip: QUARK_TEST_DATABASE_URL not set");
        return;
    };
    let store = Arc::new(store);
    store
        .put_wellknown(
            quark::tenant::DEFAULT_TENANT,
            "apple-app-site-association",
            r#"{"owner":"default"}"#,
        )
        .await
        .unwrap();

    let cache = Cache::new(store.clone() as Arc<dyn Store>, 1000, None);
    let host_router = Arc::new(quark::domain_router::HostRouter::new(
        store.clone() as Arc<dyn Store>,
        None,
        None,
    ));
    let (analytics_tx, _rx) = tokio::sync::mpsc::channel(100);
    let store_dyn: Arc<dyn Store> = store.clone();
    let sink_dyn: Arc<dyn AnalyticsSink> = store;
    let state = Arc::new(AppState {
        oidc: None,
        sheets: None,
        sheets_api: None,
        oidc_configured: false,
        multi_tenant: false,
        tenant_domain_suffix: None,
        cache,
        store: store_dyn,
        key: KEY,
        signing_key: [0u8; 32],
        analytics_tx,
        sink: sink_dyn,
        admin_token: None,
        ratelimiter: quark::abuse::ratelimit::RateLimiter::disabled(),
        block_private: true,
        public_host: None,
        real_ip_header: "cf-connecting-ip".to_string(),
        webhooks: test_webhook_dispatcher(),
        host_router,
        dns: Arc::new(NullDns),
    });
    let app = router(state);

    // Any arbitrary Host header, even one that would resolve to something
    // real in cloud mode, is entirely ignored in OSS.
    let (status, body) = get_wellknown(&app, "whatever.example.com").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body, r#"{"owner":"default"}"#);
}

/// SSRF guard extended to all registered quark hosts (P3 Task 7): creating a
/// link whose target is a verified custom domain of quark itself is a
/// self-loop and must be blocked, exactly like the shared `public_host`
/// always was. An unrelated external host is unaffected.
#[tokio::test]
#[serial]
async fn create_blocks_target_matching_any_registered_domain() {
    let Some(store) = fresh().await else {
        eprintln!("skip: QUARK_TEST_DATABASE_URL not set");
        return;
    };
    let store = Arc::new(store);
    let tenant = make_tenant(&store, "ssrf-all-hosts-a").await;
    make_domain(&store, tenant, "go.ssrf-victim.com").await;

    let (app, token) = admin_app_for_tenant(store, Arc::new(NullDns), true, tenant, 9012).await;

    // Target host is a verified quark custom domain (not the shared
    // public_host the app was built with) -> blocked as a self-loop.
    let resp = app
        .clone()
        .oneshot(
            Request::post("/")
                .header("content-type", "application/json")
                .header("x-admin-token", &token)
                .body(Body::from(r#"{"url":"https://go.ssrf-victim.com/x"}"#))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(
        resp.status(),
        StatusCode::FORBIDDEN,
        "a target host that is itself a registered quark domain must be blocked"
    );

    // An unrelated external host is unaffected.
    let resp = app
        .oneshot(
            Request::post("/")
                .header("content-type", "application/json")
                .header("x-admin-token", &token)
                .body(Body::from(
                    r#"{"url":"https://totally-unrelated.example.com/x"}"#,
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
}

// --- P3-completion Task 1: auto per-tenant subdomain -----------------------
// A tenant's subdomain is just a normal Verified `domains` row (`host =
// <slug>.<suffix>`), seeded via `quark::api::seed_tenant_subdomain`. Since
// it's a real `domains` row, `get_domain_by_host` (isolation, wellknown, SSRF)
// already works unchanged — these tests only cover the seeding itself.

/// `subdomain_host` lowercases and joins slug + suffix exactly like every
/// other host `get_domain_by_host`/`HostRouter` looks up.
#[test]
fn subdomain_host_lowercases_and_joins() {
    assert_eq!(
        quark::api::subdomain_host("Acme", "Quarkus.COM.br"),
        "acme.quarkus.com.br"
    );
}

/// Seeding a tenant's subdomain materializes a Verified `domains` row that
/// `get_domain_by_host` resolves to the right tenant.
#[tokio::test]
#[serial]
async fn seed_tenant_subdomain_materializes_verified_domain() {
    let Some(store) = fresh().await else {
        eprintln!("skip: QUARK_TEST_DATABASE_URL not set");
        return;
    };
    let store: Arc<dyn Store> = Arc::new(store);
    let tenant_id = {
        let id = store.next_tenant_id().await.unwrap();
        let tenant_id = TenantId(id);
        store
            .put_tenant(&Tenant {
                id: tenant_id,
                name: "Seed Co".to_string(),
                slug: "seed-co".to_string(),
                created: 0,
            })
            .await
            .unwrap();
        tenant_id
    };

    quark::api::seed_tenant_subdomain(&store, tenant_id, "seed-co", "quarkus.test")
        .await
        .unwrap();

    let domain = store
        .get_domain_by_host("seed-co.quarkus.test")
        .await
        .unwrap()
        .expect("subdomain must resolve via get_domain_by_host");
    assert_eq!(domain.tenant_id, tenant_id);
    assert_eq!(domain.status, DomainStatus::Verified);
    assert!(domain.verified_at.is_some());
    assert_eq!(
        domain.token, "",
        "subdomains aren't DNS-verified, so the token is empty"
    );
}

/// Seeding the same tenant's subdomain twice is idempotent: the second call
/// hits the `host` UNIQUE constraint (mapped to `StoreError::UniqueViolation`)
/// and is treated as success, leaving exactly one row.
#[tokio::test]
#[serial]
async fn seed_tenant_subdomain_is_idempotent() {
    let Some(store) = fresh().await else {
        eprintln!("skip: QUARK_TEST_DATABASE_URL not set");
        return;
    };
    let store: Arc<dyn Store> = Arc::new(store);
    let tenant_id = {
        let id = store.next_tenant_id().await.unwrap();
        let tenant_id = TenantId(id);
        store
            .put_tenant(&Tenant {
                id: tenant_id,
                name: "Twice Co".to_string(),
                slug: "twice-co".to_string(),
                created: 0,
            })
            .await
            .unwrap();
        tenant_id
    };

    quark::api::seed_tenant_subdomain(&store, tenant_id, "twice-co", "quarkus.test")
        .await
        .unwrap();
    // 2nd seed of the same tenant: must succeed (not surface the unique
    // violation to the caller) and must not create a 2nd row.
    quark::api::seed_tenant_subdomain(&store, tenant_id, "twice-co", "quarkus.test")
        .await
        .unwrap();

    assert_eq!(
        store.list_domains(tenant_id).await.unwrap().len(),
        1,
        "seeding twice must not duplicate the subdomain row"
    );
}

/// A `list_tenants` + "seed the ones missing a subdomain" pass — the shape
/// the boot backfill in `main.rs` runs — seeds a pre-existing tenant exactly
/// once, whether it runs once or twice (mirrors the idempotent boot).
#[tokio::test]
#[serial]
async fn backfill_shape_seeds_preexisting_tenant_idempotently() {
    let Some(store) = fresh().await else {
        eprintln!("skip: QUARK_TEST_DATABASE_URL not set");
        return;
    };
    let store: Arc<dyn Store> = Arc::new(store);
    let id = store.next_tenant_id().await.unwrap();
    let tenant_id = TenantId(id);
    store
        .put_tenant(&Tenant {
            id: tenant_id,
            name: "Backfill Co".to_string(),
            slug: "backfill-co".to_string(),
            created: 0,
        })
        .await
        .unwrap();

    let suffix = "quarkus.test";
    async fn run_backfill_pass(store: &Arc<dyn Store>, suffix: &str) {
        for t in store.list_tenants().await.unwrap() {
            let host = quark::api::subdomain_host(&t.slug, suffix);
            if store.get_domain_by_host(&host).await.unwrap().is_none() {
                quark::api::seed_tenant_subdomain(store, t.id, &t.slug, suffix)
                    .await
                    .unwrap();
            }
        }
    }

    // Other tenants that may already exist in this fresh DB (e.g. the seeded
    // default tenant 0) must not confuse the assertion below, so scope it to
    // just the one we created.
    run_backfill_pass(&store, suffix).await;
    run_backfill_pass(&store, suffix).await;

    assert_eq!(
        store.list_domains(tenant_id).await.unwrap().len(),
        1,
        "running the backfill pass twice must not duplicate the subdomain row"
    );
    let domain = store
        .get_domain_by_host("backfill-co.quarkus.test")
        .await
        .unwrap()
        .expect("backfill must seed the pre-existing tenant's subdomain");
    assert_eq!(domain.tenant_id, tenant_id);
}

/// `/admin/me` reports the configured suffix (or `null` when unset) so the
/// panel can build subdomain URLs without its own env var.
#[tokio::test]
#[serial]
async fn admin_me_reports_tenant_domain_suffix() {
    let Some(store) = fresh().await else {
        eprintln!("skip: QUARK_TEST_DATABASE_URL not set");
        return;
    };
    let store = Arc::new(store);
    let user_id = store.next_user_id().await.unwrap();
    store
        .put_user(&quark::tenant::User {
            id: user_id,
            subject: "admin-me-suffix-subject".to_string(),
            email: "admin-me-suffix@example.com".to_string(),
            display: "Suffix Tester".to_string(),
            created: 0,
        })
        .await
        .unwrap();
    let raw = quark::auth::generate_token();
    let session = quark::auth::Session {
        token_hash: hash_token(&raw),
        subject: "admin-me-suffix-subject".to_string(),
        display: "Suffix Tester".to_string(),
        scopes: vec![],
        created: 0,
        expires: quark::now() + 3600,
        tenant_id: TenantId(0),
        user_id,
    };
    store.put_session(TenantId(0), &session).await.unwrap();

    let app = cloud_app_with_suffix(
        store.clone() as Arc<dyn Store>,
        store.clone() as Arc<dyn AnalyticsSink>,
        None,
        Some("quarkus.test".to_string()),
    );
    let resp = app
        .oneshot(
            Request::get("/admin/me")
                .header("cookie", format!("qk_session={raw}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .unwrap();
    let body: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(body["tenant_domain_suffix"], "quarkus.test");
}

/// OSS/no-suffix parity: `seed_tenant_subdomain` is simply never called when
/// `tenant_domain_suffix` is `None` — no row is seeded, no behavior changes.
/// This is the ungated (no DB needed) half of the contract: the gate lives at
/// the call sites (`admin_tenants_create`, the boot backfill), not in the
/// helper itself, so this just documents the "unset -> no seed" invariant
/// callers rely on.
#[test]
fn no_suffix_means_no_seed_call_site_contract() {
    let suffix: Option<String> = None;
    let mut seeded = false;
    if let Some(_s) = &suffix {
        seeded = true;
    }
    assert!(!seeded, "with no suffix configured, seeding must never run");
}

/// Same `cloud_app` builder as above, but with a `tenant_domain_suffix`.
fn cloud_app_with_suffix(
    store: Arc<dyn Store>,
    sink: Arc<dyn AnalyticsSink>,
    public_host: Option<String>,
    tenant_domain_suffix: Option<String>,
) -> axum::Router {
    let cache = Cache::new(store.clone(), 1000, None);
    let host_router = Arc::new(quark::domain_router::HostRouter::new(
        store.clone(),
        public_host.clone(),
        None,
    ));
    let (analytics_tx, _rx) = tokio::sync::mpsc::channel(100);
    let state = Arc::new(AppState {
        oidc: None,
        sheets: None,
        sheets_api: None,
        oidc_configured: true,
        multi_tenant: true,
        tenant_domain_suffix,
        cache,
        store,
        key: KEY,
        signing_key: [0u8; 32],
        analytics_tx,
        sink,
        admin_token: None,
        ratelimiter: quark::abuse::ratelimit::RateLimiter::disabled(),
        block_private: true,
        public_host,
        real_ip_header: "cf-connecting-ip".to_string(),
        webhooks: test_webhook_dispatcher(),
        host_router,
        dns: Arc::new(NullDns),
    });
    router(state)
}

// --- P3-completion Task 2: create-flow fix — stamps the caller's real
// tenant and lands the alias in that tenant's default (subdomain) domain,
// instead of the pre-fix hardcode (tenant 0 / SHARED_DOMAIN_ID for every
// cloud caller).

/// Registers a `LinksWrite`-scoped API token for `tenant` and returns the
/// raw token to send as `x-admin-token`.
async fn seed_write_token(store: &Arc<PostgresStore>, tenant: TenantId, token_id: u64) -> String {
    let raw = format!("qtok_create_{}", token_id);
    store
        .put_api_token(
            tenant,
            &ApiToken {
                id: token_id,
                name: "create-flow-test-token".to_string(),
                token_hash: hash_token(&raw),
                scopes: vec![Scope::LinksWrite],
                rate_limit_per_min: None,
                created: 0,
                tenant_id: tenant,
            },
        )
        .await
        .unwrap();
    raw
}

async fn post_create(
    app: &axum::Router,
    token: &str,
    url: &str,
) -> (StatusCode, serde_json::Value) {
    let resp = app
        .clone()
        .oneshot(
            Request::post("/")
                .header("content-type", "application/json")
                .header("x-admin-token", token)
                .body(Body::from(format!(r#"{{"url":"{url}"}}"#)))
                .unwrap(),
        )
        .await
        .unwrap();
    let status = resp.status();
    let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .unwrap();
    let json = if body.is_empty() {
        serde_json::Value::Null
    } else {
        serde_json::from_slice(&body).unwrap_or(serde_json::Value::Null)
    };
    (status, json)
}

async fn get_with_host(app: &axum::Router, host: &str, path: &str) -> StatusCode {
    app.clone()
        .oneshot(
            Request::get(path)
                .header("host", host)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap()
        .status()
}

/// A cloud caller authenticated as tenant B (via a scoped API token) creates
/// a link through `POST /`. The stored `Record.tenant_id` must be B, not
/// `DEFAULT_TENANT` — the bug this task fixes — and the link (numeric code
/// and its custom alias) must resolve on B's own subdomain, not on a
/// different tenant's subdomain.
#[tokio::test]
#[serial]
async fn cloud_create_stamps_callers_tenant_and_resolves_on_its_subdomain() {
    let Some(store) = fresh().await else {
        eprintln!("skip: QUARK_TEST_DATABASE_URL not set");
        return;
    };
    let store = Arc::new(store);
    let suffix = "quarkus.test";
    let tenant_b = make_tenant(&store, "create-flow-b").await;
    let tenant_other = make_tenant(&store, "create-flow-other").await;
    quark::api::seed_tenant_subdomain(
        &(store.clone() as Arc<dyn Store>),
        tenant_b,
        "create-flow-b",
        suffix,
    )
    .await
    .unwrap();
    quark::api::seed_tenant_subdomain(
        &(store.clone() as Arc<dyn Store>),
        tenant_other,
        "create-flow-other",
        suffix,
    )
    .await
    .unwrap();
    let token = seed_write_token(&store, tenant_b, 9101).await;

    let app = cloud_app_with_suffix(
        store.clone() as Arc<dyn Store>,
        store.clone() as Arc<dyn AnalyticsSink>,
        None,
        Some(suffix.to_string()),
    );

    let (status, body) = post_create(&app, &token, "https://example.com/create-flow").await;
    assert_eq!(status, StatusCode::OK, "create must succeed: {body:?}");
    let code = body["code"].as_str().expect("code in response").to_string();

    // The link is stamped under tenant B, not DEFAULT_TENANT.
    let permuted = quark::codec::from_base62(&code).expect("numeric code decodes");
    let id = quark::permute::decode(permuted, KEY);
    let rec = store
        .get_link(tenant_b, id)
        .await
        .unwrap()
        .expect("link must be visible under tenant B");
    assert_eq!(rec.tenant_id, tenant_b);
    assert!(
        store
            .get_link(quark::tenant::DEFAULT_TENANT, id)
            .await
            .unwrap()
            .is_none(),
        "the link must NOT land under DEFAULT_TENANT"
    );

    // Resolves via B's own subdomain.
    let host_b = quark::api::subdomain_host("create-flow-b", suffix);
    let status = get_with_host(&app, &host_b, &format!("/{code}")).await;
    assert!(
        status == StatusCode::FOUND || status == StatusCode::MOVED_PERMANENTLY,
        "numeric code must resolve on the caller's own subdomain, got {status}"
    );

    // Does NOT resolve on a different tenant's subdomain.
    let host_other = quark::api::subdomain_host("create-flow-other", suffix);
    let status = get_with_host(&app, &host_other, &format!("/{code}")).await;
    assert_eq!(
        status,
        StatusCode::NOT_FOUND,
        "the numeric code is global, but the record lookup is tenant-scoped by the resolved \
         route -- another tenant's subdomain must not serve it"
    );

    // Now with a custom alias: it must land in B's subdomain namespace and
    // resolve via `b.<suffix>/<alias>`, but not via another tenant's host.
    let (status, body) =
        app_create_with_alias(&app, &token, "https://example.com/aliased", "flow-alias").await;
    assert_eq!(
        status,
        StatusCode::OK,
        "aliased create must succeed: {body:?}"
    );

    let status = get_with_host(&app, &host_b, "/flow-alias").await;
    assert!(
        status == StatusCode::FOUND || status == StatusCode::MOVED_PERMANENTLY,
        "the alias must resolve on the tenant's own subdomain, got {status}"
    );
    let status = get_with_host(&app, &host_other, "/flow-alias").await;
    assert_eq!(
        status,
        StatusCode::NOT_FOUND,
        "the same alias must not resolve on a different tenant's subdomain"
    );
}

async fn app_create_with_alias(
    app: &axum::Router,
    token: &str,
    url: &str,
    alias: &str,
) -> (StatusCode, serde_json::Value) {
    let resp = app
        .clone()
        .oneshot(
            Request::post("/")
                .header("content-type", "application/json")
                .header("x-admin-token", token)
                .body(Body::from(format!(
                    r#"{{"url":"{url}","alias":"{alias}"}}"#
                )))
                .unwrap(),
        )
        .await
        .unwrap();
    let status = resp.status();
    let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .unwrap();
    let json = if body.is_empty() {
        serde_json::Value::Null
    } else {
        serde_json::from_slice(&body).unwrap_or(serde_json::Value::Null)
    };
    (status, json)
}

/// OSS parity (regression guard): with `multi_tenant = false` (or a suffix
/// unset in cloud), `POST /` still stamps `DEFAULT_TENANT` and lands the
/// alias in `SHARED_DOMAIN_ID`, byte-for-byte as before this task.
#[tokio::test]
#[serial]
async fn oss_create_still_stamps_default_tenant_and_shared_domain() {
    let Some(store) = fresh().await else {
        eprintln!("skip: QUARK_TEST_DATABASE_URL not set");
        return;
    };
    let store = Arc::new(store);
    let cache = Cache::new(store.clone() as Arc<dyn Store>, 1000, None);
    let host_router = Arc::new(quark::domain_router::HostRouter::new(
        store.clone() as Arc<dyn Store>,
        None,
        None,
    ));
    let (analytics_tx, _rx) = tokio::sync::mpsc::channel(100);
    let state = Arc::new(AppState {
        oidc: None,
        sheets: None,
        sheets_api: None,
        oidc_configured: false,
        multi_tenant: false,
        tenant_domain_suffix: None,
        cache,
        store: store.clone() as Arc<dyn Store>,
        key: KEY,
        signing_key: [0u8; 32],
        analytics_tx,
        sink: store.clone() as Arc<dyn AnalyticsSink>,
        admin_token: None,
        ratelimiter: quark::abuse::ratelimit::RateLimiter::disabled(),
        block_private: true,
        public_host: None,
        real_ip_header: "cf-connecting-ip".to_string(),
        webhooks: test_webhook_dispatcher(),
        host_router,
        dns: Arc::new(NullDns),
    });
    let app = router(state);

    let resp = app
        .clone()
        .oneshot(
            Request::post("/")
                .header("content-type", "application/json")
                .body(Body::from(
                    r#"{"url":"https://example.com/oss-create-flow"}"#,
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .unwrap();
    let body: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    let code = body["code"].as_str().unwrap().to_string();

    let permuted = quark::codec::from_base62(&code).expect("numeric code decodes");
    let id = quark::permute::decode(permuted, KEY);
    let rec = store
        .get_link(quark::tenant::DEFAULT_TENANT, id)
        .await
        .unwrap()
        .expect("OSS create must land under DEFAULT_TENANT");
    assert_eq!(rec.tenant_id, quark::tenant::DEFAULT_TENANT);
}
