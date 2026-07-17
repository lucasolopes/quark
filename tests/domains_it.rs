use axum::body::Body;
use axum::http::{Request, StatusCode};
use quark::analytics::AnalyticsSink;
use quark::api::{router, AppState};
use quark::cache::Cache;
use quark::domain::{Domain, DomainStatus, SHARED_DOMAIN_ID};
use quark::store::postgres::PostgresStore;
use quark::store::{Record, Store};
use quark::tenant::{Tenant, TenantId};
use quark::webhooks::delivery::WebhookDispatcher;
use serial_test::serial;
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
