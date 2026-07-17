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

use axum::body::Body;
use axum::http::{Request, StatusCode};
use quark::analytics::{AnalyticsSink, ClickEvent};
use quark::api::{router, AppState};
use quark::auth::{hash_token, ApiToken, Scope, Session};
use quark::cache::Cache;
use quark::store::postgres::PostgresStore;
use quark::store::{OutboxRow, Record, Store};
use quark::tenant::{Membership, Role, TenantId};
use quark::webhooks::delivery::WebhookDispatcher;
use quark::webhooks::{EventType, SubscriptionKind, WebhookSubscription};
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

/// CRITICAL regression: `init_schema` must NOT `FORCE` RLS on
/// `click_counters`/`stats_meta`/`click_events` (analytics) or
/// `webhook_deliveries` (the cluster-wide outbox relay). Those accessors run
/// on the bare pool and never `SET LOCAL app.tenant_id`; if they were FORCE'd,
/// a non-superuser owner in cloud mode would see analytics writes/reads
/// silently return 0 rows, and `put_link_tx`/`put_alias_and_link_tx` would
/// have their delivery enqueue rejected by `WITH CHECK` (the delivery row
/// carries `tenant_id=0`, but the enclosing tenant-tx has
/// `app.tenant_id=<tenant>`, so `0 = <tenant>` fails for any non-default
/// tenant).
#[tokio::test]
async fn cloud_analytics_and_outbox_accessors_survive_force_rls() {
    let Some(url) = std::env::var("QUARK_TEST_DATABASE_URL").ok() else {
        eprintln!("skip: QUARK_TEST_DATABASE_URL not set");
        return;
    };
    let store = PostgresStore::open(&url, true).await.unwrap();
    store.reset_for_tests().await.unwrap();
    // `concrete` gives access to the `AnalyticsSink` inherent impl; `bare`
    // (the same store, as `Arc<dyn Store>`) gives access to the cluster-wide
    // outbox methods (`enqueue_deliveries`/`claim_due_deliveries`), which run
    // on the bare pool, tenant-less, exactly like production's relay.
    let concrete = Arc::new(store);
    let bare = concrete.clone() as Arc<dyn Store>;
    let t1 = bare.clone().for_tenant(TenantId(1));

    // A webhook subscription for tenant 1, subscribed to `link.created`.
    let sub_id = t1.next_webhook_id().await.unwrap();
    let sub = WebhookSubscription {
        id: sub_id,
        url: "https://enforcement.example/hook".to_string(),
        events: vec![EventType::LinkCreated],
        secret: "whsec_test".to_string(),
        active: true,
        created: 0,
        kind: SubscriptionKind::Generic,
    };
    t1.put_webhook(&sub).await.unwrap();

    // A link create that has a lifecycle delivery to enqueue: this is the
    // exact surface `put_link_tx` uses in production (redirect API's create
    // path). Before the fix, this INSERT into `webhook_deliveries` ran inside
    // the tenant-1 tx (`SET LOCAL app.tenant_id = 1`) but the delivery row's
    // `tenant_id` column defaults to 0 — under FORCE, `WITH CHECK` rejects it
    // (`0 = 1` is false).
    let link_id = 800u64;
    let delivery_key = format!("evt_enforcement.{sub_id}");
    let delivery = OutboxRow {
        delivery_key: delivery_key.clone(),
        subscription_id: sub_id,
        event_type: "link.created".to_string(),
        payload: "{}".to_string(),
        created: 0,
        next_attempt_at: 0,
        tenant_id: TenantId(1),
    };
    t1.put_link_tx(
        link_id,
        &rec("https://enforcement.example/outbox"),
        &[delivery],
    )
    .await
    .expect("put_link_tx with a non-empty delivery must not be rejected by RLS");

    // The delivery actually landed and is claimable by the relay (bare pool,
    // no `app.tenant_id` set — must not be fail-closed by FORCE either).
    let claimed = bare.claim_due_deliveries(1, 10).await.unwrap();
    let d = claimed
        .iter()
        .find(|d| d.delivery_key == delivery_key)
        .expect("delivery enqueued by put_link_tx must be claimable by the outbox relay");
    assert_eq!(
        d.tenant_id,
        TenantId(1),
        "the claimed row must carry the subscription's tenant, so the relay \
         resolves it via get_webhook(TenantId(1), ...) and not DEFAULT_TENANT"
    );

    // Analytics: `record_batch`/`stats` run on the bare pool too and must not
    // be fail-closed by FORCE.
    let click = ClickEvent {
        id: link_id,
        event_id: String::new(),
        ts: 1,
        referer: None,
        country: Some("BR".into()),
        user_agent: None,
        city: None,
        bot: false,
        ip: None,
        fbc: None,
        variant: None,
        tenant_id: 0,
    };
    concrete
        .record_batch(&[click])
        .await
        .expect("record_batch must not be rejected by RLS");
    let stats = concrete
        .stats(link_id)
        .await
        .expect("stats must not be rejected by RLS");
    assert!(
        stats.is_some(),
        "stats must read back the just-recorded click (no spurious 0-rows under FORCE)"
    );
}

/// CRITICAL regression: `init_schema` must NOT `FORCE` RLS on `api_tokens` or
/// `sessions`. Auth runs *before* a tenant is known (a bearer token/session
/// cookie is the thing that resolves the tenant), so these lookups run on the
/// bare pool with no `app.tenant_id` set. If they were FORCE'd, login and API
/// auth would silently fail closed (0 rows) for every tenant in cloud mode.
#[tokio::test]
async fn cloud_hash_lookups_survive_force_rls() {
    let Some(url) = std::env::var("QUARK_TEST_DATABASE_URL").ok() else {
        eprintln!("skip: QUARK_TEST_DATABASE_URL not set");
        return;
    };
    let store = PostgresStore::open(&url, true).await.unwrap();
    store.reset_for_tests().await.unwrap();
    let bare = Arc::new(store) as Arc<dyn Store>;
    let t1 = bare.clone().for_tenant(TenantId(1));

    // Put via the tenant-scoped write path (as production does when issuing
    // a token/session for a logged-in tenant), then look up by hash on the
    // bare pool (as production's auth middleware does before a tenant is
    // known).
    let token_id = t1.next_api_token_id().await.unwrap();
    let token = ApiToken {
        id: token_id,
        name: "enforcement-ci".into(),
        token_hash: "enforcement_token_hash".into(),
        scopes: vec![Scope::LinksRead],
        rate_limit_per_min: None,
        created: 0,
        tenant_id: TenantId(1),
    };
    t1.put_api_token(&token).await.unwrap();

    let got_token = bare
        .get_api_token_by_hash("enforcement_token_hash")
        .await
        .unwrap();
    assert_eq!(
        got_token.map(|t| t.id),
        Some(token_id),
        "get_api_token_by_hash must find the token from the bare pool \
         (api_tokens must not be FORCE'd)"
    );

    let session = Session {
        token_hash: "enforcement_session_hash".into(),
        subject: "sub-enforcement".into(),
        display: "enforcement@example.com".into(),
        scopes: vec![Scope::LinksRead],
        created: 0,
        expires: 1_000_000_000,
        tenant_id: TenantId(1),
        user_id: 0,
    };
    t1.put_session(&session).await.unwrap();

    let got_session = bare
        .get_session_by_hash("enforcement_session_hash", 0)
        .await
        .unwrap();
    assert_eq!(
        got_session.map(|s| s.subject),
        Some("sub-enforcement".to_string()),
        "get_session_by_hash must find the session from the bare pool \
         (sessions must not be FORCE'd)"
    );
}

/// A `WebhookDispatcher` whose receiver is dropped: `emit` silently no-ops.
fn test_webhook_dispatcher() -> Arc<WebhookDispatcher> {
    let (tx, _rx) = tokio::sync::mpsc::channel(1);
    Arc::new(WebhookDispatcher::new(
        tx,
        Arc::new(std::sync::atomic::AtomicBool::new(false)),
        Arc::new(std::sync::atomic::AtomicBool::new(false)),
    ))
}

/// Builds a cloud-mode `AppState` (`multi_tenant = true`, `oidc_configured =
/// true`, no env admin token) over the given Postgres-backed store/sink, so
/// `admin_guard`'s OIDC-session branch derives scopes from membership role.
fn cloud_state(store: Arc<dyn Store>, sink: Arc<dyn AnalyticsSink>) -> Arc<AppState> {
    let cache = Cache::new(store.clone(), 1000, None);
    let host_router = Arc::new(quark::domain_router::HostRouter::new(
        store.clone(),
        None,
        None,
    ));
    let (analytics_tx, _rx) = tokio::sync::mpsc::channel(100);
    Arc::new(AppState {
        oidc: None,
        sheets: None,
        sheets_api: None,
        oidc_configured: true,
        multi_tenant: true,
        tenant_domain_suffix: None,
        oidc_tenants: quark::oidc::TenantOidcCache::new(),
        cache,
        store,
        key: 0x1234,
        signing_key: [0u8; 32],
        analytics_tx,
        sink,
        admin_token: None,
        ratelimiter: quark::abuse::ratelimit::RateLimiter::disabled(),
        block_private: true,
        public_host: None,
        real_ip_header: "cf-connecting-ip".to_string(),
        webhooks: test_webhook_dispatcher(),
        host_router,
        dns: std::sync::Arc::new(quark::dns::NullDns),
    })
}

/// Puts a login session (keyed by the hash of `raw`) for `user_id` in `tenant`.
async fn put_login_session(store: &Arc<dyn Store>, raw: &str, user_id: u64, tenant: TenantId) {
    let session = Session {
        token_hash: hash_token(raw),
        subject: format!("sub-{user_id}"),
        display: format!("user-{user_id}@example.com"),
        // Deliberately grant a broad stored scope: in cloud mode `admin_guard`
        // must IGNORE this and use the membership role, so a session minted with
        // Full here still cannot exceed the caller's role in the tenant.
        scopes: vec![Scope::Full],
        created: 0,
        // Far future, but within i64 (the BIGINT column): u64::MAX would wrap
        // to -1 and read back as already-expired.
        expires: 4_000_000_000,
        tenant_id: tenant,
        user_id,
    };
    store.put_session(tenant, &session).await.unwrap();
}

/// CRITICAL (P2b Task 5): in cloud mode the OIDC-session branch of `admin_guard`
/// authorizes by the caller's MEMBERSHIP ROLE in the current tenant
/// (`session.tenant_id`), not by the stored `session.scopes`. A Viewer can read
/// links but not write them; a session whose user has no membership in the
/// current tenant is treated as insufficient (403), even though the session row
/// itself carries `Scope::Full`.
#[tokio::test]
async fn admin_guard_role_scopes_in_cloud() {
    let Some(url) = std::env::var("QUARK_TEST_DATABASE_URL").ok() else {
        eprintln!("skip: QUARK_TEST_DATABASE_URL not set");
        return;
    };
    let store = PostgresStore::open(&url, true).await.unwrap();
    store.reset_for_tests().await.unwrap();
    let pg = Arc::new(store);
    let store_dyn: Arc<dyn Store> = pg.clone();
    let sink_dyn: Arc<dyn AnalyticsSink> = pg.clone();

    // A Viewer in tenant 1.
    let viewer_tenant = TenantId(1);
    let viewer_id = 900u64;
    store_dyn
        .put_membership(&Membership {
            user_id: viewer_id,
            tenant_id: viewer_tenant,
            role: Role::Viewer,
            created: 0,
        })
        .await
        .unwrap();
    let viewer_raw = "viewer-session-token";
    put_login_session(&store_dyn, viewer_raw, viewer_id, viewer_tenant).await;

    // A user with NO membership in the tenant its session points at.
    let orphan_id = 901u64;
    let orphan_tenant = TenantId(2);
    let orphan_raw = "orphan-session-token";
    put_login_session(&store_dyn, orphan_raw, orphan_id, orphan_tenant).await;

    let app = router(cloud_state(store_dyn, sink_dyn));

    // Viewer -> LinksRead (GET /admin/links) is allowed (200).
    let resp = app
        .clone()
        .oneshot(
            Request::get("/admin/links")
                .header("cookie", format!("qk_session={viewer_raw}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(
        resp.status(),
        StatusCode::OK,
        "Viewer role must cover LinksRead in the current tenant"
    );

    // Viewer -> LinksWrite (POST / create) is denied (403), even though the
    // stored session scope is Full.
    let resp = app
        .clone()
        .oneshot(
            Request::post("/")
                .header("content-type", "application/json")
                .header("cookie", format!("qk_session={viewer_raw}"))
                .body(Body::from(r#"{"url":"https://example.com/viewer-write"}"#))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(
        resp.status(),
        StatusCode::FORBIDDEN,
        "Viewer role must NOT cover LinksWrite (stored session.scopes=Full is ignored in cloud)"
    );

    // No membership in the session's tenant -> any required scope is 403.
    let resp = app
        .clone()
        .oneshot(
            Request::get("/admin/links")
                .header("cookie", format!("qk_session={orphan_raw}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(
        resp.status(),
        StatusCode::FORBIDDEN,
        "a session whose user has no membership in the current tenant must never authorize"
    );
}
