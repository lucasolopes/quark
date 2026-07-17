use quark::oidc::TenantOidcConfig;
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

fn cfg(tenant_id: TenantId, issuer: &str) -> TenantOidcConfig {
    TenantOidcConfig {
        tenant_id,
        issuer: issuer.to_string(),
        client_id: "acme-client".to_string(),
        client_secret: "s3cr3t-refresh-me-never".to_string(),
        scopes: vec!["openid".to_string(), "profile".to_string()],
        admin_claim: "groups".to_string(),
        admin_value: "acme-admins".to_string(),
        readonly_value: "acme-viewers".to_string(),
        post_login_url: Some("/dashboard".to_string()),
    }
}

/// Puts a config for tenant A; the tenant-scoped read (`get_oidc_config`,
/// the admin CRUD path) sees it back byte-for-byte, secret included (it
/// round-trips through the JSONB `blob`, mirroring `sheets_connection`'s
/// plaintext-refresh-token precedent).
#[tokio::test]
#[serial]
async fn put_then_get_round_trips_including_secret() {
    let Some(store) = fresh().await else {
        eprintln!("skip: QUARK_TEST_DATABASE_URL not set");
        return;
    };
    let a = make_tenant(&store, "oidc-tenant-a").await;
    let config = cfg(a, "https://idp.acme.example");
    store.put_oidc_config(&config).await.unwrap();

    let got = store
        .get_oidc_config(a)
        .await
        .unwrap()
        .expect("config must exist");
    assert_eq!(got, config);
}

/// Tenant B's own tenant-scoped read never sees tenant A's config: the
/// isolation the RLS + `WHERE tenant_id` predicate is supposed to give every
/// other tenant-owned table.
#[tokio::test]
#[serial]
async fn tenant_scoped_read_does_not_leak_across_tenants() {
    let Some(store) = fresh().await else {
        eprintln!("skip: QUARK_TEST_DATABASE_URL not set");
        return;
    };
    let a = make_tenant(&store, "oidc-isolation-a").await;
    let b = make_tenant(&store, "oidc-isolation-b").await;
    store
        .put_oidc_config(&cfg(a, "https://idp.acme.example"))
        .await
        .unwrap();

    assert!(store.get_oidc_config(a).await.unwrap().is_some());
    assert!(
        store.get_oidc_config(b).await.unwrap().is_none(),
        "tenant B must not see tenant A's OIDC config"
    );
}

/// `get_oidc_config_bare` (the login/callback path, before any RLS context
/// exists) also returns tenant A's config, unscoped by a transaction.
#[tokio::test]
#[serial]
async fn bare_read_returns_config_before_any_tenant_context() {
    let Some(store) = fresh().await else {
        eprintln!("skip: QUARK_TEST_DATABASE_URL not set");
        return;
    };
    let a = make_tenant(&store, "oidc-bare-a").await;
    let config = cfg(a, "https://idp.acme.example");
    store.put_oidc_config(&config).await.unwrap();

    let got = store
        .get_oidc_config_bare(a)
        .await
        .unwrap()
        .expect("bare read must find the config");
    assert_eq!(got, config);
}

/// Putting a second config for the same tenant replaces the first (UPSERT on
/// the UNIQUE `tenant_id`), leaving exactly one row and the newer values.
#[tokio::test]
#[serial]
async fn put_upserts_replacing_not_duplicating() {
    let Some(store) = fresh().await else {
        eprintln!("skip: QUARK_TEST_DATABASE_URL not set");
        return;
    };
    let a = make_tenant(&store, "oidc-upsert-a").await;
    store
        .put_oidc_config(&cfg(a, "https://idp-v1.acme.example"))
        .await
        .unwrap();
    let mut updated = cfg(a, "https://idp-v2.acme.example");
    updated.client_secret = "rotated-secret".to_string();
    store.put_oidc_config(&updated).await.unwrap();

    let got = store
        .get_oidc_config(a)
        .await
        .unwrap()
        .expect("config must exist");
    assert_eq!(got, updated);
    assert_eq!(got.issuer, "https://idp-v2.acme.example");
    assert_eq!(got.client_secret, "rotated-secret");
}

/// `delete_oidc_config` removes the row; a subsequent read (either path) sees
/// nothing, and deleting again is not an error.
#[tokio::test]
#[serial]
async fn delete_removes_config_both_read_paths() {
    let Some(store) = fresh().await else {
        eprintln!("skip: QUARK_TEST_DATABASE_URL not set");
        return;
    };
    let a = make_tenant(&store, "oidc-delete-a").await;
    store
        .put_oidc_config(&cfg(a, "https://idp.acme.example"))
        .await
        .unwrap();

    store.delete_oidc_config(a).await.unwrap();
    assert!(store.get_oidc_config(a).await.unwrap().is_none());
    assert!(store.get_oidc_config_bare(a).await.unwrap().is_none());
    // Deleting a config that no longer exists is not an error.
    store.delete_oidc_config(a).await.unwrap();
}

/// `get_tenant_by_slug` resolves the tenant `/admin/login?org=<slug>` needs,
/// and an unknown slug is `None` rather than an error.
#[tokio::test]
#[serial]
async fn get_tenant_by_slug_resolves_or_none() {
    let Some(store) = fresh().await else {
        eprintln!("skip: QUARK_TEST_DATABASE_URL not set");
        return;
    };
    let a = make_tenant(&store, "acme").await;

    let found = store
        .get_tenant_by_slug("acme")
        .await
        .unwrap()
        .expect("must resolve the seeded slug");
    assert_eq!(found.id, a);
    assert_eq!(found.slug, "acme");

    assert!(store
        .get_tenant_by_slug("no-such-tenant")
        .await
        .unwrap()
        .is_none());
}
