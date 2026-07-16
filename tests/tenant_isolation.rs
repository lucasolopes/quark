//! Multi-tenancy P1a isolation tests. The LMDB arm is always exercised; the
//! Postgres arm is gated on `QUARK_TEST_DATABASE_URL` (skipped when unset).

use quark::store::{open_store, Record, Store};
use quark::tenant::TenantId;
use std::sync::Arc;

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

// --- LMDB arm (no gating) ---

#[tokio::test]
async fn lmdb_scans_are_bounded_to_tenant() {
    let dir = tempfile::tempdir().unwrap();
    let store = open_store(dir.path()).await.unwrap();
    let a = store.clone().for_tenant(TenantId(1));
    let b = store.clone().for_tenant(TenantId(2));
    let r = rec("https://example.com");
    a.put_link(1, &r).await.unwrap();
    a.put_link(2, &r).await.unwrap();
    b.put_link(3, &r).await.unwrap();
    assert_eq!(a.list_links(None, 100, None, None).await.unwrap().len(), 2);
    assert_eq!(b.list_links(None, 100, None, None).await.unwrap().len(), 1);
    // Cross-tenant reads at another tenant's id return None.
    assert!(b.get_link(1).await.unwrap().is_none());
    assert!(a.get_link(3).await.unwrap().is_none());
}

#[tokio::test]
async fn lmdb_default_tenant_is_seeded_and_migration_marker_set() {
    let dir = tempfile::tempdir().unwrap();
    let store = open_store(dir.path()).await.unwrap();
    // The default tenant always exists on a fresh DB.
    assert!(store
        .get_tenant(quark::tenant::DEFAULT_TENANT)
        .await
        .unwrap()
        .is_some());
    // Re-opening the same path is a no-op (idempotent migration) and preserves
    // data written under the default tenant.
    let d = store.clone().for_tenant(quark::tenant::DEFAULT_TENANT);
    d.put_link(42, &rec("https://kept.example")).await.unwrap();
    drop(store);
    let store2 = open_store(dir.path()).await.unwrap();
    let d2 = store2.clone().for_tenant(quark::tenant::DEFAULT_TENANT);
    assert_eq!(
        d2.get_link(42).await.unwrap().unwrap().url,
        "https://kept.example"
    );
}

#[tokio::test]
async fn lmdb_identity_round_trips() {
    let dir = tempfile::tempdir().unwrap();
    let store = open_store(dir.path()).await.unwrap();
    let uid = store.next_user_id().await.unwrap();
    let user = quark::tenant::User {
        id: uid,
        subject: "oidc|abc".into(),
        email: "a@e.com".into(),
        display: "A".into(),
        created: 1,
    };
    store.put_user(&user).await.unwrap();
    assert_eq!(
        store.get_user_by_subject("oidc|abc").await.unwrap().unwrap().id,
        uid
    );
    let m = quark::tenant::Membership {
        user_id: uid,
        tenant_id: TenantId(7),
        role: quark::tenant::Role::Admin,
        created: 1,
    };
    store.put_membership(&m).await.unwrap();
    assert_eq!(
        store.get_membership(uid, TenantId(7)).await.unwrap().unwrap().role,
        quark::tenant::Role::Admin
    );
    assert_eq!(store.list_memberships_for_user(uid).await.unwrap().len(), 1);
}

// --- Postgres arm (gated on QUARK_TEST_DATABASE_URL) ---

#[tokio::test]
async fn migration_seeds_default_tenant_and_columns() {
    let Some(url) = std::env::var("QUARK_TEST_DATABASE_URL").ok() else {
        return;
    };
    let store = quark::store::open_postgres(&url).await.unwrap();
    // default tenant exists after init_schema
    let t = store.get_tenant(TenantId(0)).await.unwrap();
    assert!(t.is_some(), "default tenant 0 must be seeded by init_schema");
}

#[tokio::test]
async fn pg_two_tenants_do_not_see_each_others_links() {
    let Some(url) = std::env::var("QUARK_TEST_DATABASE_URL").ok() else {
        return;
    };
    let store = quark::store::open_postgres(&url).await.unwrap();
    store.reset_for_tests().await.unwrap();
    let dyn_store: Arc<dyn Store> = store.clone();
    let a = dyn_store.clone().for_tenant(TenantId(1));
    let b = dyn_store.clone().for_tenant(TenantId(2));
    let r = rec("https://example.com");
    a.put_link(500, &r).await.unwrap();
    assert!(a.get_link(500).await.unwrap().is_some());
    assert!(b.get_link(500).await.unwrap().is_none());
    assert_eq!(b.list_links(None, 100, None, None).await.unwrap().len(), 0);
    assert_eq!(a.list_links(None, 100, None, None).await.unwrap().len(), 1);
}
