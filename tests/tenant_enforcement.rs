//! Multi-tenancy P2a enforcement: in cloud mode (`multi_tenant = true`) every
//! tenant-owned query runs inside a transaction that first did
//! `SET LOCAL app.tenant_id`, and the tenant-owned tables carry
//! `FORCE ROW LEVEL SECURITY`. That makes cross-tenant access fail closed at the
//! database, independently of the app-level `WHERE tenant_id` predicate (which
//! stays as belt-and-suspenders).
//!
//! Gated on `QUARK_TEST_DATABASE_URL` (needs a live Postgres). When it is unset
//! the test early-returns; the controller runs the gated arm against real
//! Postgres. Correct by construction either way.

use quark::store::{Record, Store};
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

#[tokio::test]
async fn cloud_force_rls_is_fail_closed() {
    let Some(url) = std::env::var("QUARK_TEST_DATABASE_URL").ok() else {
        eprintln!("skip: QUARK_TEST_DATABASE_URL not set");
        return;
    };
    // multi_tenant = true -> FORCE RLS + per-tenant tx routing.
    let store = quark::store::postgres::PostgresStore::open(&url, true)
        .await
        .unwrap();
    store.reset_for_tests().await.unwrap();
    let a = Arc::new(store) as Arc<dyn Store>;
    let t1 = a.clone().for_tenant(TenantId(1));
    let t2 = a.clone().for_tenant(TenantId(2));

    let r = rec("https://enforcement.example/p2a");
    t1.put_link(700, &r).await.unwrap();

    // Enforced by RLS (the tenant-tx sets app.tenant_id and the table is
    // FORCE'd), not merely by the WHERE predicate: tenant 1 sees its row,
    // tenant 2 sees nothing.
    assert!(
        t1.get_link(700).await.unwrap().is_some(),
        "owning tenant must see its own link"
    );
    assert!(
        t2.get_link(700).await.unwrap().is_none(),
        "other tenant must not see the link"
    );
    assert_eq!(
        t2.list_links(None, 100, None, None).await.unwrap().len(),
        0,
        "other tenant must list zero links"
    );
    assert_eq!(
        t1.list_links(None, 100, None, None).await.unwrap().len(),
        1,
        "owning tenant must list its own link"
    );
}
