//! Store-level tests for SSO email-domain discovery (LUC-57, Task 1).
//! Postgres-gated on `QUARK_TEST_DATABASE_URL`; skips when unset.
use quark::domain::DomainStatus;
use quark::sso::SsoEmailDomain;
use quark::store::postgres::PostgresStore;
use quark::store::Store;
use quark::tenant::{Tenant, TenantId};
use serial_test::serial;

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

async fn put(store: &PostgresStore, tenant: TenantId, domain: &str) -> u64 {
    let id = store.next_sso_domain_id().await.unwrap();
    store
        .put_sso_domain(&SsoEmailDomain {
            id,
            tenant_id: tenant,
            domain: domain.to_string(),
            token: format!("tok-{id}"),
            status: DomainStatus::Pending,
            created: 0,
            verified_at: None,
        })
        .await
        .unwrap();
    id
}

/// A pending domain round-trips through the bare lookup, and flipping it to
/// verified persists the status + timestamp.
#[tokio::test]
#[serial]
async fn put_get_bare_and_verify() {
    let Some(store) = fresh().await else {
        eprintln!("skip: QUARK_TEST_DATABASE_URL not set");
        return;
    };
    let t = make_tenant(&store, "sso-a").await;
    let id = put(&store, t, "acme.com").await;

    let got = store
        .get_sso_domain_bare("acme.com")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(got.id, id);
    assert_eq!(got.tenant_id, t);
    assert_eq!(got.status, DomainStatus::Pending);
    assert!(got.verified_at.is_none());

    store
        .set_sso_domain_status(t, id, DomainStatus::Verified, Some(42))
        .await
        .unwrap();
    let got = store
        .get_sso_domain_bare("acme.com")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(got.status, DomainStatus::Verified);
    assert_eq!(got.verified_at, Some(42));

    // Unknown domain -> None.
    assert!(store
        .get_sso_domain_bare("nope.com")
        .await
        .unwrap()
        .is_none());
}

/// `domain` is UNIQUE across tenants: a second tenant cannot claim a domain a
/// first tenant already owns, and the first tenant's row is untouched.
#[tokio::test]
#[serial]
async fn domain_is_unique_across_tenants() {
    let Some(store) = fresh().await else {
        eprintln!("skip: QUARK_TEST_DATABASE_URL not set");
        return;
    };
    let a = make_tenant(&store, "sso-owner").await;
    let b = make_tenant(&store, "sso-squatter").await;
    let a_id = put(&store, a, "shared.com").await;

    let b_new = store.next_sso_domain_id().await.unwrap();
    let err = store
        .put_sso_domain(&SsoEmailDomain {
            id: b_new,
            tenant_id: b,
            domain: "shared.com".to_string(),
            token: "tok-b".to_string(),
            status: DomainStatus::Pending,
            created: 0,
            verified_at: None,
        })
        .await;
    assert!(
        err.is_err(),
        "second tenant claiming the same domain must fail"
    );

    // The original owner's row is unchanged.
    let got = store
        .get_sso_domain_bare("shared.com")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(got.id, a_id);
    assert_eq!(got.tenant_id, a);
}

/// `list_sso_domains` and `get_sso_domain` are tenant-scoped: tenant B never
/// sees tenant A's rows through the scoped accessors.
#[tokio::test]
#[serial]
async fn scoped_accessors_are_tenant_isolated() {
    let Some(store) = fresh().await else {
        eprintln!("skip: QUARK_TEST_DATABASE_URL not set");
        return;
    };
    let a = make_tenant(&store, "sso-list-a").await;
    let b = make_tenant(&store, "sso-list-b").await;
    let a_id = put(&store, a, "a.com").await;
    put(&store, b, "b.com").await;

    let a_list = store.list_sso_domains(a).await.unwrap();
    assert_eq!(a_list.len(), 1);
    assert_eq!(a_list[0].domain, "a.com");

    // A's row is visible to A by id, invisible to B.
    assert!(store.get_sso_domain(a, a_id).await.unwrap().is_some());
    assert!(store.get_sso_domain(b, a_id).await.unwrap().is_none());
}

/// Delete removes the row (and frees the domain for a future claim).
#[tokio::test]
#[serial]
async fn delete_removes_the_row() {
    let Some(store) = fresh().await else {
        eprintln!("skip: QUARK_TEST_DATABASE_URL not set");
        return;
    };
    let t = make_tenant(&store, "sso-del").await;
    let id = put(&store, t, "gone.com").await;
    store.delete_sso_domain(t, id).await.unwrap();
    assert!(store
        .get_sso_domain_bare("gone.com")
        .await
        .unwrap()
        .is_none());
}
