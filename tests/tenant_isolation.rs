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
        tenant_id: quark::tenant::DEFAULT_TENANT,
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
    assert_eq!(
        a.list_links(None, 100, None, None, false)
            .await
            .unwrap()
            .len(),
        2
    );
    assert_eq!(
        b.list_links(None, 100, None, None, false)
            .await
            .unwrap()
            .len(),
        1
    );
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
        store
            .get_user_by_subject("oidc|abc")
            .await
            .unwrap()
            .unwrap()
            .id,
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
        store
            .get_membership(uid, TenantId(7))
            .await
            .unwrap()
            .unwrap()
            .role,
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
    assert!(
        t.is_some(),
        "default tenant 0 must be seeded by init_schema"
    );
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
    assert_eq!(
        b.list_links(None, 100, None, None, false)
            .await
            .unwrap()
            .len(),
        0
    );
    assert_eq!(
        a.list_links(None, 100, None, None, false)
            .await
            .unwrap()
            .len(),
        1
    );
}

// --- Full entity sweep: every tenant-owned method, both backends ---

/// Exercises every tenant-owned entity for cross-tenant leakage: tenant A
/// writes one of each, tenant B must see none of it (get -> None, list ->
/// empty), and A must still see its own write. Shared by the LMDB and
/// Postgres arms below so both backends run the identical battery.
async fn assert_full_isolation(store: Arc<dyn Store>) {
    let a = store.clone().for_tenant(TenantId(11));
    let b = store.clone().for_tenant(TenantId(22));

    // --- link ---
    let r = rec("https://a.example.com/full-sweep");
    a.put_link(9001, &r).await.unwrap();
    assert!(
        a.get_link(9001).await.unwrap().is_some(),
        "A must see its own link"
    );
    assert!(
        b.get_link(9001).await.unwrap().is_none(),
        "B must not see A's link"
    );
    assert_eq!(
        a.list_links(None, 100, None, None, false)
            .await
            .unwrap()
            .len(),
        1,
        "A must list its own link"
    );
    assert!(
        b.list_links(None, 100, None, None, false)
            .await
            .unwrap()
            .is_empty(),
        "B must not list A's link"
    );

    // --- alias ---
    // P3 Task 2: the alias namespace moved from per-tenant to per-domain, so
    // isolation here is asserted across two different `domain_id`s (11, 22)
    // rather than across tenant A/B (the shared domain, `SHARED_DOMAIN_ID`,
    // is deliberately NOT isolated: see `alias_namespace_is_per_domain` in
    // `tests/domains_it.rs`).
    a.put_alias_and_link(11, "full-sweep-alias", 9001, &r)
        .await
        .unwrap();
    assert!(
        a.get_alias(11, "full-sweep-alias").await.unwrap().is_some(),
        "A must see its own alias in its own domain"
    );
    assert!(
        a.get_alias(22, "full-sweep-alias").await.unwrap().is_none(),
        "the same alias must not resolve in a different domain"
    );

    // --- webhook ---
    let webhook = quark::webhooks::WebhookSubscription {
        id: 9002,
        url: "https://hooks.example.com/full-sweep".into(),
        events: vec![quark::webhooks::EventType::LinkCreated],
        secret: "shh".into(),
        active: true,
        created: 0,
        kind: quark::webhooks::SubscriptionKind::Generic,
        label: None,
        connector_id: None,
        external_id: None,
        last_delivery_at: None,
        last_delivery_status: Default::default(),
    };
    a.put_webhook(&webhook).await.unwrap();
    assert!(
        a.get_webhook(9002).await.unwrap().is_some(),
        "A must see its own webhook"
    );
    assert!(
        b.get_webhook(9002).await.unwrap().is_none(),
        "B must not see A's webhook"
    );
    assert_eq!(
        a.list_webhooks().await.unwrap().len(),
        1,
        "A must list its own webhook"
    );
    assert!(
        b.list_webhooks().await.unwrap().is_empty(),
        "B must not list A's webhook"
    );

    // --- api_token (list is tenant-scoped; hash-lookup is deliberately
    // tenant-less by design, so it is not asserted here) ---
    let token = quark::auth::ApiToken {
        id: 9003,
        name: "full-sweep-token".into(),
        token_hash: "full-sweep-hash".into(),
        scopes: vec![quark::auth::Scope::Full],
        rate_limit_per_min: None,
        created: 0,
        tenant_id: quark::tenant::DEFAULT_TENANT,
    };
    a.put_api_token(&token).await.unwrap();
    assert_eq!(
        a.list_api_tokens().await.unwrap().len(),
        1,
        "A must list its own api token"
    );
    assert!(
        b.list_api_tokens().await.unwrap().is_empty(),
        "B must not list A's api token"
    );

    // --- pixel ---
    let pixel = quark::pixel::PixelConfig {
        id: 9004,
        provider: quark::pixel::Provider::Ga4,
        credentials: quark::pixel::PixelCredentials::default(),
        active: true,
        created: 0,
        last_forward_at: None,
        last_forward_status: Default::default(),
    };
    a.put_pixel(&pixel).await.unwrap();
    assert!(
        a.get_pixel(9004).await.unwrap().is_some(),
        "A must see its own pixel"
    );
    assert!(
        b.get_pixel(9004).await.unwrap().is_none(),
        "B must not see A's pixel"
    );
    assert_eq!(
        a.list_pixels().await.unwrap().len(),
        1,
        "A must list its own pixel"
    );
    assert!(
        b.list_pixels().await.unwrap().is_empty(),
        "B must not list A's pixel"
    );

    // --- wellknown ---
    a.put_wellknown("apple-app-site-association", "{\"full\":\"sweep\"}")
        .await
        .unwrap();
    assert!(
        a.get_wellknown("apple-app-site-association")
            .await
            .unwrap()
            .is_some(),
        "A must see its own wellknown document"
    );
    assert!(
        b.get_wellknown("apple-app-site-association")
            .await
            .unwrap()
            .is_none(),
        "B must not see A's wellknown document"
    );

    // --- link_health ---
    let health = quark::store::LinkHealth {
        checked_at: 1,
        status: Some(200),
        healthy: true,
    };
    a.put_link_health(9001, &health).await.unwrap();
    assert_eq!(
        a.list_link_health().await.unwrap().len(),
        1,
        "A must list its own link health"
    );
    assert!(
        b.list_link_health().await.unwrap().is_empty(),
        "B must not list A's link health"
    );

    // --- visits ---
    a.bump_visits(9001).await.unwrap();
    assert_eq!(
        a.visits(9001).await.unwrap(),
        1,
        "A must see its own visit count"
    );
    assert_eq!(
        b.visits(9001).await.unwrap(),
        0,
        "B must not see A's visit count"
    );

    // --- sheets_connection ---
    let conn = quark::sheets::SheetsConnection {
        refresh_token: "full-sweep-refresh".into(),
        email: "a@full-sweep.example.com".into(),
        spreadsheet_id: None,
        last_sync: None,
        last_status: quark::sheets::SyncStatus::Never,
    };
    a.put_sheets_connection(&conn).await.unwrap();
    assert!(
        a.get_sheets_connection().await.unwrap().is_some(),
        "A must see its own sheets connection"
    );
    assert!(
        b.get_sheets_connection().await.unwrap().is_none(),
        "B must not see A's sheets connection"
    );
}

#[tokio::test]
async fn every_tenant_owned_entity_is_isolated() {
    // LMDB arm: always runs.
    let dir = tempfile::tempdir().unwrap();
    let lmdb_store = open_store(dir.path()).await.unwrap();
    assert_full_isolation(lmdb_store).await;

    // Postgres arm: only when a test database is configured.
    if let Ok(url) = std::env::var("QUARK_TEST_DATABASE_URL") {
        let pg_store = quark::store::open_postgres(&url).await.unwrap();
        pg_store.reset_for_tests().await.unwrap();
        let dyn_store: Arc<dyn Store> = pg_store.clone();
        assert_full_isolation(dyn_store).await;
    }
}

// --- P1b: credentials carry tenant + user ---

#[tokio::test]
async fn lmdb_token_and_session_carry_tenant_and_user() {
    let dir = tempfile::tempdir().unwrap();
    let store = quark::store::open_store(dir.path()).await.unwrap();
    let t = quark::tenant::TenantId(0);

    let tok = quark::auth::ApiToken {
        id: 1,
        name: "t".into(),
        token_hash: "h1".into(),
        scopes: vec![quark::auth::Scope::Full],
        rate_limit_per_min: None,
        created: 0,
        tenant_id: t,
    };
    store.put_api_token(t, &tok).await.unwrap();
    let got = store.get_api_token_by_hash("h1").await.unwrap().unwrap();
    assert_eq!(got.tenant_id, t);

    let sess = quark::auth::Session {
        token_hash: "s1".into(),
        subject: "sub".into(),
        display: "d".into(),
        scopes: vec![quark::auth::Scope::Full],
        created: 0,
        expires: u64::MAX,
        tenant_id: t,
        user_id: 7,
        id_token: None,
    };
    store.put_session(t, &sess).await.unwrap();
    let gs = store.get_session_by_hash("s1", 0).await.unwrap().unwrap();
    assert_eq!(gs.tenant_id, t);
    assert_eq!(gs.user_id, 7);
}

#[tokio::test]
async fn pg_token_and_session_carry_tenant_and_user() {
    let Some(url) = std::env::var("QUARK_TEST_DATABASE_URL").ok() else {
        eprintln!("skip: QUARK_TEST_DATABASE_URL not set");
        return;
    };
    let store = quark::store::open_postgres(&url).await.unwrap();
    store.reset_for_tests().await.unwrap();
    let t = quark::tenant::TenantId(0);

    let tok = quark::auth::ApiToken {
        id: 1,
        name: "t".into(),
        token_hash: "h1-pg".into(),
        scopes: vec![quark::auth::Scope::Full],
        rate_limit_per_min: None,
        created: 0,
        tenant_id: t,
    };
    store.put_api_token(t, &tok).await.unwrap();
    let got = store.get_api_token_by_hash("h1-pg").await.unwrap().unwrap();
    assert_eq!(got.tenant_id, t);

    let sess = quark::auth::Session {
        token_hash: "s1-pg".into(),
        subject: "sub".into(),
        display: "d".into(),
        // i64::MAX (far future), not u64::MAX: Postgres stores `expires` as
        // BIGINT (i64), so u64::MAX would wrap to -1 and fail the `expires > now`
        // filter. Production expiries are now()+TTL, always well within i64.
        scopes: vec![quark::auth::Scope::Full],
        created: 0,
        expires: i64::MAX as u64,
        tenant_id: t,
        user_id: 7,
        id_token: None,
    };
    store.put_session(t, &sess).await.unwrap();
    let gs = store
        .get_session_by_hash("s1-pg", 0)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(gs.tenant_id, t);
    assert_eq!(gs.user_id, 7);
}

// --- P1b Task 5: tenant-correct PKs for sheets_connection and
// wellknown_documents (closes a P1a carry-over: the old PKs were `singleton`
// / `name` alone, which cannot hold two tenants' rows at once). ---

#[tokio::test]
async fn pg_wellknown_and_sheets_pks_are_tenant_correct() {
    let Some(url) = std::env::var("QUARK_TEST_DATABASE_URL").ok() else {
        eprintln!("skip: QUARK_TEST_DATABASE_URL not set");
        return;
    };
    let store = quark::store::open_postgres(&url).await.unwrap();
    store.reset_for_tests().await.unwrap();
    let dyn_store: Arc<dyn Store> = store.clone();
    let a = dyn_store.clone().for_tenant(TenantId(31));
    let b = dyn_store.clone().for_tenant(TenantId(32));

    // Two tenants, same wellknown document name -> both coexist under the
    // new (tenant_id, name) PK; the old `name`-only PK would reject the
    // second insert or clobber the first tenant's row.
    a.put_wellknown("apple-app-site-association", "{\"tenant\":31}")
        .await
        .unwrap();
    b.put_wellknown("apple-app-site-association", "{\"tenant\":32}")
        .await
        .unwrap();
    assert_eq!(
        a.get_wellknown("apple-app-site-association")
            .await
            .unwrap()
            .unwrap(),
        "{\"tenant\":31}"
    );
    assert_eq!(
        b.get_wellknown("apple-app-site-association")
            .await
            .unwrap()
            .unwrap(),
        "{\"tenant\":32}"
    );

    // sheets_connection: put twice for the same tenant -> upsert (one row),
    // reads back the latest, under the new `(tenant_id)` PK (no `singleton`
    // column).
    let conn1 = quark::sheets::SheetsConnection {
        refresh_token: "first".into(),
        email: "a@example.com".into(),
        spreadsheet_id: None,
        last_sync: None,
        last_status: quark::sheets::SyncStatus::Never,
    };
    let conn2 = quark::sheets::SheetsConnection {
        refresh_token: "second".into(),
        email: "a@example.com".into(),
        spreadsheet_id: None,
        last_sync: None,
        last_status: quark::sheets::SyncStatus::Never,
    };
    a.put_sheets_connection(&conn1).await.unwrap();
    a.put_sheets_connection(&conn2).await.unwrap();
    assert_eq!(
        a.get_sheets_connection()
            .await
            .unwrap()
            .unwrap()
            .refresh_token,
        "second"
    );
    assert!(b.get_sheets_connection().await.unwrap().is_none());

    // Re-run init_schema (boot-time migration) twice more to confirm the PK
    // migration is idempotent (no panics/errors on a schema that already has
    // the new PKs).
    quark::store::open_postgres(&url).await.unwrap();
    quark::store::open_postgres(&url).await.unwrap();
}
