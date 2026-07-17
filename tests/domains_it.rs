use quark::domain::{Domain, DomainStatus, SHARED_DOMAIN_ID};
use quark::store::postgres::PostgresStore;
use quark::store::{Record, Store};
use quark::tenant::{Tenant, TenantId};
use serial_test::serial;

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
