use axum::body::Body;
use axum::http::{Request, StatusCode};
use quark::analytics::AnalyticsSink;
use quark::api::{router, AppState};
use quark::auth::{generate_token, hash_token, Session};
use quark::cache::Cache;
use quark::store::{postgres::PostgresStore, Store};
use quark::tenant::{TenantId, User};
use serial_test::serial;
use std::sync::Arc;
use tower::ServiceExt;

async fn fresh() -> Option<PostgresStore> {
    let url = std::env::var("QUARK_TEST_DATABASE_URL").ok()?;
    let s = PostgresStore::open(&url, true).await.unwrap();
    s.reset_for_tests().await.unwrap();
    Some(s)
}

// ids are >=1 and monotonic, never 0 (0 is the seeded default tenant).
#[tokio::test]
#[serial]
async fn next_tenant_id_starts_above_default() {
    let Some(store) = fresh().await else {
        return;
    };
    let a = store.next_tenant_id().await.unwrap();
    let b = store.next_tenant_id().await.unwrap();
    assert!(
        a >= 1 && b > a,
        "tenant ids must be >=1 (0 is the default tenant) and monotonic"
    );
}

/// A `WebhookDispatcher` for tests that don't exercise webhooks: the receiver
/// is dropped immediately, so `emit` silently no-ops.
fn test_webhook_dispatcher() -> Arc<quark::webhooks::delivery::WebhookDispatcher> {
    let (tx, _rx) = tokio::sync::mpsc::channel(1);
    Arc::new(quark::webhooks::delivery::WebhookDispatcher::new(
        tx,
        Arc::new(std::sync::atomic::AtomicBool::new(false)),
        Arc::new(std::sync::atomic::AtomicBool::new(false)),
    ))
}

/// Builds a full quark router over `store` with a given `multi_tenant` mode.
fn app_over(
    store: Arc<dyn Store>,
    sink: Arc<dyn AnalyticsSink>,
    multi_tenant: bool,
) -> axum::Router {
    let cache = Cache::new(store.clone(), 1000, None);
    let (analytics_tx, _rx) = tokio::sync::mpsc::channel(100);
    let state = Arc::new(AppState {
        oidc: None,
        sheets: None,
        sheets_api: None,
        oidc_configured: true,
        multi_tenant,
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
    });
    router(state)
}

/// Seeds a user + a session for it, returning the raw session-cookie value.
async fn seed_session(store: &PostgresStore, subject: &str) -> (u64, String) {
    let user_id = store.next_user_id().await.unwrap();
    store
        .put_user(&User {
            id: user_id,
            subject: subject.to_string(),
            email: format!("{subject}@example.com"),
            display: subject.to_string(),
            created: 0,
        })
        .await
        .unwrap();
    let raw = generate_token();
    let session = Session {
        token_hash: hash_token(&raw),
        subject: subject.to_string(),
        display: subject.to_string(),
        scopes: vec![],
        created: 0,
        expires: quark::now() + 3600,
        tenant_id: TenantId(0),
        user_id,
    };
    store.put_session(TenantId(0), &session).await.unwrap();
    (user_id, raw)
}

/// `POST /admin/tenants` (cloud): any authenticated OIDC user (even with zero
/// memberships) can self-serve a workspace. Verifies the created tenant, the
/// Owner membership, the session's tenant switching to it, the 409 on a
/// duplicate slug, and the 404 in OSS mode.
#[tokio::test]
#[serial]
async fn create_tenant_self_serve() {
    let Some(store) = fresh().await else {
        eprintln!("skip: QUARK_TEST_DATABASE_URL not set");
        return;
    };
    let store = Arc::new(store);
    let (user_id, raw) = seed_session(&store, "create-tenant-subject").await;

    let app = app_over(
        store.clone() as Arc<dyn Store>,
        store.clone() as Arc<dyn AnalyticsSink>,
        true,
    );

    let resp = app
        .clone()
        .oneshot(
            Request::post("/admin/tenants")
                .header("content-type", "application/json")
                .header("cookie", format!("qk_session={raw}"))
                .body(Body::from(r#"{"name":"Acme","slug":"acme-co"}"#))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .unwrap();
    let tenant: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    let tenant_id = tenant["id"].as_u64().unwrap();
    assert!(tenant_id >= 1, "new tenant id must not be the default 0");
    assert_eq!(tenant["name"], "Acme");
    assert_eq!(tenant["slug"], "acme-co");

    // The caller is now Owner on the new tenant.
    let membership = store
        .get_membership(user_id, TenantId(tenant_id))
        .await
        .unwrap()
        .expect("membership must exist");
    assert_eq!(membership.role, quark::tenant::Role::Owner);

    // The session's current tenant switched to the new one.
    let session = store
        .get_session_by_hash(&hash_token(&raw), quark::now())
        .await
        .unwrap()
        .expect("session must still exist");
    assert_eq!(session.tenant_id, TenantId(tenant_id));

    // A 2nd create with the SAME slug -> 409 (unique slug).
    let resp = app
        .clone()
        .oneshot(
            Request::post("/admin/tenants")
                .header("content-type", "application/json")
                .header("cookie", format!("qk_session={raw}"))
                .body(Body::from(r#"{"name":"Acme Again","slug":"acme-co"}"#))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::CONFLICT);

    // In OSS mode (multi_tenant = false) the route is 404.
    let oss_app = app_over(
        store.clone() as Arc<dyn Store>,
        store as Arc<dyn AnalyticsSink>,
        false,
    );
    let resp = oss_app
        .oneshot(
            Request::post("/admin/tenants")
                .header("content-type", "application/json")
                .header("cookie", format!("qk_session={raw}"))
                .body(Body::from(r#"{"name":"Nope","slug":"nope"}"#))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
}

/// No session cookie at all -> 401, not 404 (cloud endpoint exists, just
/// unauthenticated).
#[tokio::test]
#[serial]
async fn create_tenant_requires_session() {
    let Some(store) = fresh().await else {
        eprintln!("skip: QUARK_TEST_DATABASE_URL not set");
        return;
    };
    let store = Arc::new(store);
    let app = app_over(
        store.clone() as Arc<dyn Store>,
        store as Arc<dyn AnalyticsSink>,
        true,
    );
    let resp = app
        .oneshot(
            Request::post("/admin/tenants")
                .header("content-type", "application/json")
                .body(Body::from(r#"{"name":"Acme","slug":"acme-co"}"#))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
}

/// `POST /admin/workspace/switch` (cloud): switching to a tenant the caller
/// IS a member of succeeds and re-points the session; switching to one they
/// are NOT a member of is refused with 403 and — the security invariant —
/// leaves the session's current tenant unchanged. Also checks OSS 404.
#[tokio::test]
#[serial]
async fn workspace_switch_checks_membership() {
    let Some(store) = fresh().await else {
        eprintln!("skip: QUARK_TEST_DATABASE_URL not set");
        return;
    };
    let store = Arc::new(store);
    let (user_id, raw) = seed_session(&store, "switch-subject").await;

    // Tenant A: caller is Owner.
    let tenant_a = store.next_tenant_id().await.unwrap();
    store
        .put_tenant(&quark::tenant::Tenant {
            id: TenantId(tenant_a),
            name: "A".to_string(),
            slug: "workspace-a".to_string(),
            created: 0,
        })
        .await
        .unwrap();
    store
        .put_membership(&quark::tenant::Membership {
            user_id,
            tenant_id: TenantId(tenant_a),
            role: quark::tenant::Role::Owner,
            created: 0,
        })
        .await
        .unwrap();

    // Tenant B: exists, but the caller has NO membership in it.
    let tenant_b = store.next_tenant_id().await.unwrap();
    store
        .put_tenant(&quark::tenant::Tenant {
            id: TenantId(tenant_b),
            name: "B".to_string(),
            slug: "workspace-b".to_string(),
            created: 0,
        })
        .await
        .unwrap();

    let app = app_over(
        store.clone() as Arc<dyn Store>,
        store.clone() as Arc<dyn AnalyticsSink>,
        true,
    );

    // Switch to A (member) -> 200, session now points at A.
    let resp = app
        .clone()
        .oneshot(
            Request::post("/admin/workspace/switch")
                .header("content-type", "application/json")
                .header("cookie", format!("qk_session={raw}"))
                .body(Body::from(format!(r#"{{"tenant_id":{tenant_a}}}"#)))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let session = store
        .get_session_by_hash(&hash_token(&raw), quark::now())
        .await
        .unwrap()
        .expect("session must still exist");
    assert_eq!(session.tenant_id, TenantId(tenant_a));

    // Switch to B (NOT a member) -> 403, session's tenant UNCHANGED (still A).
    let resp = app
        .clone()
        .oneshot(
            Request::post("/admin/workspace/switch")
                .header("content-type", "application/json")
                .header("cookie", format!("qk_session={raw}"))
                .body(Body::from(format!(r#"{{"tenant_id":{tenant_b}}}"#)))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::FORBIDDEN);
    let session = store
        .get_session_by_hash(&hash_token(&raw), quark::now())
        .await
        .unwrap()
        .expect("session must still exist");
    assert_eq!(
        session.tenant_id,
        TenantId(tenant_a),
        "a 403 must NOT mutate the session"
    );

    // OSS mode (multi_tenant = false) -> 404.
    let oss_app = app_over(
        store.clone() as Arc<dyn Store>,
        store as Arc<dyn AnalyticsSink>,
        false,
    );
    let resp = oss_app
        .oneshot(
            Request::post("/admin/workspace/switch")
                .header("content-type", "application/json")
                .header("cookie", format!("qk_session={raw}"))
                .body(Body::from(format!(r#"{{"tenant_id":{tenant_a}}}"#)))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
}
