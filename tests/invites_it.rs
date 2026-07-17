//! P2c Task 1: `invites` table + store methods, cloud-only. Mirrors the
//! non-superuser, PG-gated harness in `tests/domains_it.rs`.
use quark::auth::hash_token;
use quark::invite::Invite;
use quark::store::postgres::PostgresStore;
use quark::store::Store;
use quark::tenant::{Role, Tenant, TenantId};
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

async fn make_invite(
    store: &PostgresStore,
    tenant: TenantId,
    email: &str,
    raw_token: &str,
    created: u64,
    expires: u64,
) -> u64 {
    let id = store.next_invite_id().await.unwrap();
    store
        .create_invite(&Invite {
            id,
            tenant_id: tenant,
            email: email.to_string(),
            role: Role::Member,
            token_hash: hash_token(raw_token),
            invited_by: 1,
            created,
            expires,
            accepted_at: None,
            accepted_by: None,
        })
        .await
        .unwrap();
    id
}

/// The full lifecycle: create -> hash lookup finds it -> accept -> hash
/// lookup no longer finds it (accepted invites are invisible).
#[tokio::test]
#[serial]
async fn invite_accept_hides_it_from_hash_lookup() {
    let Some(store) = fresh().await else {
        eprintln!("skip: QUARK_TEST_DATABASE_URL not set");
        return;
    };
    let tenant = make_tenant(&store, "invites-accept-a").await;
    let id = make_invite(&store, tenant, "new@acme.com", "raw-token-1", 100, 1_000).await;

    let found = store
        .get_invite_by_hash(&hash_token("raw-token-1"), 200)
        .await
        .unwrap()
        .expect("pending invite must be found by its token hash");
    assert_eq!(found.id, id);
    assert_eq!(found.tenant_id, tenant);
    assert_eq!(found.email, "new@acme.com");
    assert_eq!(found.role, Role::Member);
    assert_eq!(found.accepted_at, None);
    assert_eq!(found.accepted_by, None);

    store.mark_invite_accepted(id, 42, 300).await.unwrap();

    assert!(
        store
            .get_invite_by_hash(&hash_token("raw-token-1"), 400)
            .await
            .unwrap()
            .is_none(),
        "an accepted invite must not be returned by hash lookup"
    );
}

/// An invite whose `expires` is before `now` is invisible to the hash lookup,
/// even though it was never accepted.
#[tokio::test]
#[serial]
async fn expired_invite_is_invisible_to_hash_lookup() {
    let Some(store) = fresh().await else {
        eprintln!("skip: QUARK_TEST_DATABASE_URL not set");
        return;
    };
    let tenant = make_tenant(&store, "invites-expired-a").await;
    make_invite(&store, tenant, "late@acme.com", "raw-token-2", 100, 500).await;

    assert!(
        store
            .get_invite_by_hash(&hash_token("raw-token-2"), 501)
            .await
            .unwrap()
            .is_none(),
        "an expired invite must not be returned by hash lookup"
    );

    // Still not expired one tick earlier: `expires >= now` is inclusive.
    assert!(store
        .get_invite_by_hash(&hash_token("raw-token-2"), 500)
        .await
        .unwrap()
        .is_some());
}

/// `list_invites` is tenant-scoped: tenant B never sees tenant A's pending
/// invite.
#[tokio::test]
#[serial]
async fn list_invites_is_tenant_scoped() {
    let Some(store) = fresh().await else {
        eprintln!("skip: QUARK_TEST_DATABASE_URL not set");
        return;
    };
    let tenant_a = make_tenant(&store, "invites-list-a").await;
    let tenant_b = make_tenant(&store, "invites-list-b").await;
    make_invite(&store, tenant_a, "a@acme.com", "raw-token-3", 100, 1_000).await;

    let list_a = store.list_invites(tenant_a).await.unwrap();
    assert_eq!(list_a.len(), 1);
    assert_eq!(list_a[0].email, "a@acme.com");

    let list_b = store.list_invites(tenant_b).await.unwrap();
    assert!(
        list_b.is_empty(),
        "tenant B must not see tenant A's pending invite"
    );
}

/// `delete_invite` is tenant-scoped: tenant B cannot delete tenant A's
/// invite by id, and tenant A's own delete removes it from its list.
#[tokio::test]
#[serial]
async fn delete_invite_is_tenant_scoped() {
    let Some(store) = fresh().await else {
        eprintln!("skip: QUARK_TEST_DATABASE_URL not set");
        return;
    };
    let tenant_a = make_tenant(&store, "invites-delete-a").await;
    let tenant_b = make_tenant(&store, "invites-delete-b").await;
    let id = make_invite(&store, tenant_a, "del@acme.com", "raw-token-4", 100, 1_000).await;

    store.delete_invite(tenant_b, id).await.unwrap();
    assert_eq!(
        store.list_invites(tenant_a).await.unwrap().len(),
        1,
        "tenant B's delete attempt must not remove tenant A's invite"
    );

    store.delete_invite(tenant_a, id).await.unwrap();
    assert!(store.list_invites(tenant_a).await.unwrap().is_empty());
}
