use axum::body::Body;
use axum::http::{Request, StatusCode};
use quark::analytics::AnalyticsSink;
use quark::api::{router, AppState};
use quark::auth::{generate_token, hash_token, Session};
use quark::cache::Cache;
use quark::store::{open_backends, postgres::PostgresStore, Store};
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
    app_over_with_suffix(store, sink, multi_tenant, None)
}

/// Same as `app_over`, but with a configurable `tenant_domain_suffix`
/// (multi-tenancy P3-completion): the auto per-tenant subdomain seed on
/// `admin_tenants_create` only fires when this is `Some`.
fn app_over_with_suffix(
    store: Arc<dyn Store>,
    sink: Arc<dyn AnalyticsSink>,
    multi_tenant: bool,
    tenant_domain_suffix: Option<String>,
) -> axum::Router {
    app_over_full(store, sink, multi_tenant, tenant_domain_suffix, None, None)
}

/// Same as `app_over`, but with a `KeycloakAdmin` wired in (multi-tenancy
/// P2e): the auto realm provisioning on `admin_tenants_create` only fires
/// when `keycloak` is `Some`, exactly like the real `AppState`.
fn app_over_with_keycloak(
    store: Arc<dyn Store>,
    sink: Arc<dyn AnalyticsSink>,
    multi_tenant: bool,
    keycloak: Option<Arc<dyn quark::keycloak::KeycloakAdmin>>,
    keycloak_base_url: Option<String>,
) -> axum::Router {
    app_over_full(store, sink, multi_tenant, None, keycloak, keycloak_base_url)
}

/// Shared builder behind `app_over`/`app_over_with_suffix`/
/// `app_over_with_keycloak`: every knob those defer to the default (`None`)
/// funnels through here.
fn app_over_full(
    store: Arc<dyn Store>,
    sink: Arc<dyn AnalyticsSink>,
    multi_tenant: bool,
    tenant_domain_suffix: Option<String>,
    keycloak: Option<Arc<dyn quark::keycloak::KeycloakAdmin>>,
    keycloak_base_url: Option<String>,
) -> axum::Router {
    let cache = Cache::new(store.clone(), 1000, None);
    let host_router = Arc::new(quark::domain_router::HostRouter::new(
        store.clone(),
        None,
        None,
    ));
    let (analytics_tx, _rx) = tokio::sync::mpsc::channel(100);
    let state = Arc::new(AppState {
        oidc: None,
        sheets: None,
        sheets_api: None,
        oidc_configured: true,
        multi_tenant,
        tenant_domain_suffix,
        oidc_tenants: quark::oidc::TenantOidcCache::new(),
        keycloak,
        keycloak_base_url,
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

    // Re-read through the actual API contract (not just the store): a fresh
    // GET /admin/me with the same cookie must still report the original
    // current_tenant (A), proving the 403 is invisible to the session as
    // observed by every other endpoint, not merely at the storage layer.
    let resp = app
        .clone()
        .oneshot(
            Request::get("/admin/me")
                .header("cookie", format!("qk_session={raw}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .unwrap();
    let me: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(
        me["current_tenant"], tenant_a,
        "GET /admin/me after a rejected switch must still show the original workspace"
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

/// `GET /admin/me` (cloud): a fresh user with zero memberships gets
/// `memberships: []` and `current_tenant: null` -- the signal the frontend
/// uses to route to onboarding. After self-serving a workspace (making the
/// caller Owner there), `/admin/me` lists that membership and `current_tenant`
/// points at it.
#[tokio::test]
#[serial]
async fn me_memberships() {
    let Some(store) = fresh().await else {
        eprintln!("skip: QUARK_TEST_DATABASE_URL not set");
        return;
    };
    let store = Arc::new(store);
    let (_user_id, raw) = seed_session(&store, "me-memberships-subject").await;

    let app = app_over(
        store.clone() as Arc<dyn Store>,
        store.clone() as Arc<dyn AnalyticsSink>,
        true,
    );

    // Fresh user, zero memberships -> onboarding signal.
    let resp = app
        .clone()
        .oneshot(
            Request::get("/admin/me")
                .header("cookie", format!("qk_session={raw}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .unwrap();
    let me: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(me["authenticated"], true);
    assert_eq!(me["memberships"], serde_json::json!([]));
    assert_eq!(me["current_tenant"], serde_json::Value::Null);

    // Self-serve a workspace -> caller becomes Owner there.
    let resp = app
        .clone()
        .oneshot(
            Request::post("/admin/tenants")
                .header("content-type", "application/json")
                .header("cookie", format!("qk_session={raw}"))
                .body(Body::from(
                    r#"{"name":"Acme","slug":"me-memberships-acme"}"#,
                ))
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

    let resp = app
        .clone()
        .oneshot(
            Request::get("/admin/me")
                .header("cookie", format!("qk_session={raw}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .unwrap();
    let me: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(me["current_tenant"], tenant_id);
    let memberships = me["memberships"].as_array().unwrap();
    assert_eq!(memberships.len(), 1);
    assert_eq!(memberships[0]["tenant_id"], tenant_id);
    assert_eq!(memberships[0]["name"], "Acme");
    assert_eq!(memberships[0]["slug"], "me-memberships-acme");
    assert_eq!(memberships[0]["role"], "owner");
}

// --- OSS parity (multi_tenant = false) -------------------------------------
//
// These run over an LMDB-backed store, never gated on `QUARK_TEST_DATABASE_URL`:
// the whole point is that OSS deployments (no Postgres, no cloud flag) keep
// behaving exactly as before P2b, so this must always run, not just when a
// test Postgres happens to be configured.

/// `POST /admin/tenants` and `POST /admin/workspace/switch` are cloud-only
/// surfaces: with `multi_tenant = false` both are 404, matching the
/// pre-existing OSS contract (endpoint doesn't exist), and this holds even
/// with no session/credential presented at all -- the flag check runs before
/// authentication.
#[tokio::test]
async fn oss_workspace_endpoints_are_404() {
    let dir = tempfile::tempdir().unwrap();
    let (store, sink) = open_backends(dir.path(), false).await.unwrap();
    let app = app_over(store, sink, false);

    let resp = app
        .clone()
        .oneshot(
            Request::post("/admin/tenants")
                .header("content-type", "application/json")
                .body(Body::from(r#"{"name":"Acme","slug":"acme-co"}"#))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);

    let resp = app
        .oneshot(
            Request::post("/admin/workspace/switch")
                .header("content-type", "application/json")
                .body(Body::from(r#"{"tenant_id":1}"#))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
}

/// `GET /admin/me` shape is unchanged in OSS: the pre-P2b fields
/// (`authenticated`, `display`, `scopes`, `oidc_enabled`) are exactly as
/// before, and the P2b additions degrade to empty/null rather than leaking
/// cloud concepts into a single-tenant deployment -- `memberships` is always
/// `[]` and `current_tenant` is always `null`, even though the logged-in user
/// really does hold a tenant-0 membership underneath (OSS just never surfaces
/// it here).
#[tokio::test]
async fn oss_admin_me_shape_unchanged() {
    let dir = tempfile::tempdir().unwrap();
    let (store, sink) = open_backends(dir.path(), false).await.unwrap();

    let user_id = store.next_user_id().await.unwrap();
    store
        .put_user(&User {
            id: user_id,
            subject: "oss-me-subject".to_string(),
            email: "oss-me-subject@example.com".to_string(),
            display: "OSS Me".to_string(),
            created: 0,
        })
        .await
        .unwrap();
    // The login path (`ensure_user_and_membership`) would create this in
    // DEFAULT_TENANT for every OSS login; put it directly here to prove
    // `/admin/me` still reports the OSS shape even though a real membership
    // exists underneath.
    store
        .put_membership(&quark::tenant::Membership {
            user_id,
            tenant_id: TenantId(0),
            role: quark::tenant::Role::Admin,
            created: 0,
        })
        .await
        .unwrap();
    let raw = generate_token();
    let session = Session {
        token_hash: hash_token(&raw),
        subject: "oss-me-subject".to_string(),
        display: "OSS Me".to_string(),
        scopes: vec![quark::auth::Scope::Full],
        created: 0,
        expires: quark::now() + 3600,
        tenant_id: TenantId(0),
        user_id,
    };
    store.put_session(TenantId(0), &session).await.unwrap();

    let app = app_over(store.clone(), sink, false);
    let resp = app
        .oneshot(
            Request::get("/admin/me")
                .header("cookie", format!("qk_session={raw}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .unwrap();
    let me: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(me["authenticated"], true);
    assert_eq!(me["display"], "OSS Me");
    assert_eq!(me["oidc_enabled"], false);
    assert_eq!(
        me["memberships"],
        serde_json::json!([]),
        "OSS must never surface tenant memberships, even though one exists in the store"
    );
    assert_eq!(
        me["current_tenant"],
        serde_json::Value::Null,
        "OSS must never surface a current_tenant"
    );
}

/// `POST /admin/tenants` (cloud, with `QUARK_TENANT_DOMAIN_SUFFIX` set): the
/// self-serve create seeds the tenant's automatic subdomain as a Verified
/// `domains` row, end to end through the real handler (not just the store
/// helper directly). `get_domain_by_host` resolves it to the new tenant.
#[tokio::test]
#[serial]
async fn create_tenant_seeds_subdomain_when_suffix_configured() {
    let Some(store) = fresh().await else {
        eprintln!("skip: QUARK_TEST_DATABASE_URL not set");
        return;
    };
    let store = Arc::new(store);
    let (_user_id, raw) = seed_session(&store, "subdomain-seed-subject").await;

    let app = app_over_with_suffix(
        store.clone() as Arc<dyn Store>,
        store.clone() as Arc<dyn AnalyticsSink>,
        true,
        Some("quarkus.test".to_string()),
    );

    let resp = app
        .oneshot(
            Request::post("/admin/tenants")
                .header("content-type", "application/json")
                .header("cookie", format!("qk_session={raw}"))
                .body(Body::from(
                    r#"{"name":"Subdomain Co","slug":"subdomain-co"}"#,
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    let domain = store
        .get_domain_by_host("subdomain-co.quarkus.test")
        .await
        .unwrap()
        .expect("creating a tenant must seed its automatic subdomain");
    assert_eq!(domain.status, quark::domain::DomainStatus::Verified);
}

/// OSS/no-suffix parity: without `tenant_domain_suffix` configured, creating
/// a tenant never seeds any subdomain row — the gate is the config, not the
/// mode.
#[tokio::test]
#[serial]
async fn create_tenant_seeds_no_subdomain_without_suffix() {
    let Some(store) = fresh().await else {
        eprintln!("skip: QUARK_TEST_DATABASE_URL not set");
        return;
    };
    let store = Arc::new(store);
    let (_user_id, raw) = seed_session(&store, "no-suffix-subject").await;

    let app = app_over(
        store.clone() as Arc<dyn Store>,
        store.clone() as Arc<dyn AnalyticsSink>,
        true,
    );

    let resp = app
        .oneshot(
            Request::post("/admin/tenants")
                .header("content-type", "application/json")
                .header("cookie", format!("qk_session={raw}"))
                .body(Body::from(
                    r#"{"name":"No Suffix Co","slug":"no-suffix-co"}"#,
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    assert!(
        store
            .get_domain_by_host("no-suffix-co.quarkus.test")
            .await
            .unwrap()
            .is_none(),
        "with no suffix configured, no subdomain row must ever be seeded"
    );
}

// --- Keycloak realm provisioning on create + boot backfill (multi-tenancy
// P2e Task 2) ---

/// `POST /admin/tenants` (cloud, with a `KeycloakAdmin` configured): creating
/// a tenant provisions its Keycloak realm end to end, in order — realm,
/// client (pointed at the shared `/admin/callback` redirect), groups+mapper,
/// the owner user (in `quark-admins`, email = the creating Principal's), the
/// set-password email — and then writes the tenant's `oidc_config` with the
/// issuer derived from the realm, `client_id` "quark", an EMPTY
/// `client_secret` (public client + PKCE, no secret to store), and the
/// admin/readonly group values. Every `KeycloakAdmin` call is idempotent
/// (`409` = ok in the real client), so this order is safe to replay.
#[tokio::test]
#[serial]
async fn create_tenant_provisions_keycloak_realm() {
    let Some(store) = fresh().await else {
        eprintln!("skip: QUARK_TEST_DATABASE_URL not set");
        return;
    };
    let store = Arc::new(store);
    let (_user_id, raw) = seed_session(&store, "kc-provision-subject").await;

    let mock = Arc::new(quark::keycloak::testing::MockKeycloakAdmin::default());
    mock.set_next_user_id("kc-user-1");
    let app = app_over_with_keycloak(
        store.clone() as Arc<dyn Store>,
        store.clone() as Arc<dyn AnalyticsSink>,
        true,
        Some(mock.clone() as Arc<dyn quark::keycloak::KeycloakAdmin>),
        Some("https://kc.example.com".to_string()),
    );

    let resp = app
        .oneshot(
            Request::post("/admin/tenants")
                .header("content-type", "application/json")
                .header("cookie", format!("qk_session={raw}"))
                .body(Body::from(r#"{"name":"Kc Co","slug":"kc-provision-co"}"#))
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

    assert_eq!(
        mock.calls(),
        vec![
            "ensure_realm(kc-provision-co)".to_string(),
            "ensure_client(kc-provision-co,)".to_string(),
            "ensure_groups_and_mapper(kc-provision-co)".to_string(),
            "ensure_user(kc-provision-co,kc-provision-subject@example.com,quark-admins)"
                .to_string(),
            "send_set_password_email(kc-provision-co,kc-user-1)".to_string(),
        ],
        "provisioning must run realm -> client -> groups/mapper -> owner user -> set-password email, in that order"
    );

    let cfg = store
        .get_oidc_config_bare(TenantId(tenant_id))
        .await
        .unwrap()
        .expect("creating a tenant with keycloak configured must write its oidc_config");
    assert_eq!(cfg.issuer, "https://kc.example.com/realms/kc-provision-co");
    assert_eq!(cfg.client_id, "quark");
    assert_eq!(
        cfg.client_secret, "",
        "the quark client is public + PKCE: no client secret must ever be stored"
    );
    assert_eq!(cfg.admin_claim, "groups");
    assert_eq!(cfg.admin_value, "quark-admins");
    assert_eq!(cfg.readonly_value, "quark-readers");
    assert!(
        cfg.required_value.is_some(),
        "the group gate must default-closed for an auto-provisioned tenant"
    );
}

/// OSS/cloud-without-Keycloak parity: with `st.keycloak = None`, creating a
/// tenant makes zero Keycloak calls (there is no mock to call, by
/// construction) and writes no `oidc_config` at all — byte-for-byte the same
/// outcome as before this task (P2d-A).
#[tokio::test]
#[serial]
async fn create_tenant_without_keycloak_writes_no_oidc_config() {
    let Some(store) = fresh().await else {
        eprintln!("skip: QUARK_TEST_DATABASE_URL not set");
        return;
    };
    let store = Arc::new(store);
    let (_user_id, raw) = seed_session(&store, "no-kc-subject").await;

    let app = app_over_with_keycloak(
        store.clone() as Arc<dyn Store>,
        store.clone() as Arc<dyn AnalyticsSink>,
        true,
        None,
        None,
    );

    let resp = app
        .oneshot(
            Request::post("/admin/tenants")
                .header("content-type", "application/json")
                .header("cookie", format!("qk_session={raw}"))
                .body(Body::from(r#"{"name":"No Kc Co","slug":"no-kc-co"}"#))
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

    assert!(
        store
            .get_oidc_config_bare(TenantId(tenant_id))
            .await
            .unwrap()
            .is_none(),
        "without a configured KeycloakAdmin, no oidc_config must ever be written"
    );
}

/// Boot backfill (`quark::api::backfill_keycloak_provisioning`): a tenant
/// created before Keycloak was configured (no `oidc_config` yet) gets
/// provisioned on the backfill pass; a tenant that already has one is left
/// alone (no extra calls), and the count returned matches how many were
/// actually provisioned.
#[tokio::test]
#[serial]
async fn backfill_provisions_tenants_missing_oidc_config() {
    let Some(store) = fresh().await else {
        eprintln!("skip: QUARK_TEST_DATABASE_URL not set");
        return;
    };
    let store: Arc<dyn Store> = Arc::new(store);

    // `reset_for_tests` always seeds the default tenant (id 0); mark it
    // already-provisioned too so the assertions below are only about the two
    // tenants this test cares about.
    store
        .put_oidc_config(&quark::oidc::TenantOidcConfig {
            tenant_id: TenantId(0),
            issuer: "https://kc.example.com/realms/default".to_string(),
            client_id: "quark".to_string(),
            client_secret: String::new(),
            scopes: vec!["openid".to_string()],
            admin_claim: "groups".to_string(),
            admin_value: "quark-admins".to_string(),
            readonly_value: "quark-readers".to_string(),
            required_value: Some("quark-readers".to_string()),
            post_login_url: None,
        })
        .await
        .unwrap();

    let missing_id = store.next_tenant_id().await.unwrap();
    store
        .put_tenant(&quark::tenant::Tenant {
            id: TenantId(missing_id),
            name: "Backfill Missing".to_string(),
            slug: "backfill-missing".to_string(),
            created: 0,
        })
        .await
        .unwrap();

    let already_id = store.next_tenant_id().await.unwrap();
    store
        .put_tenant(&quark::tenant::Tenant {
            id: TenantId(already_id),
            name: "Backfill Already".to_string(),
            slug: "backfill-already".to_string(),
            created: 0,
        })
        .await
        .unwrap();
    store
        .put_oidc_config(&quark::oidc::TenantOidcConfig {
            tenant_id: TenantId(already_id),
            issuer: "https://kc.example.com/realms/backfill-already".to_string(),
            client_id: "quark".to_string(),
            client_secret: String::new(),
            scopes: vec!["openid".to_string()],
            admin_claim: "groups".to_string(),
            admin_value: "quark-admins".to_string(),
            readonly_value: "quark-readers".to_string(),
            required_value: Some("quark-readers".to_string()),
            post_login_url: None,
        })
        .await
        .unwrap();

    let mock: Arc<dyn quark::keycloak::KeycloakAdmin> =
        Arc::new(quark::keycloak::testing::MockKeycloakAdmin::default());

    let provisioned =
        quark::api::backfill_keycloak_provisioning(&store, &mock, "https://kc.example.com")
            .await
            .unwrap();
    assert_eq!(
        provisioned, 1,
        "only the tenant missing an oidc_config must be (re-)provisioned"
    );

    let cfg = store
        .get_oidc_config_bare(TenantId(missing_id))
        .await
        .unwrap()
        .expect("backfill must write the oidc_config for the tenant that lacked one");
    assert_eq!(cfg.issuer, "https://kc.example.com/realms/backfill-missing");

    // Idempotent: running the backfill again provisions nothing new.
    let provisioned_again =
        quark::api::backfill_keycloak_provisioning(&store, &mock, "https://kc.example.com")
            .await
            .unwrap();
    assert_eq!(provisioned_again, 0);
}

/// Security sweep: provisioning a tenant only ever touches that tenant's own
/// slug. Two different users self-serve two different tenants against the
/// same `KeycloakAdmin`; every recorded call carries exactly one of the two
/// slugs, never both, and each tenant's own slug shows up in its own calls.
#[tokio::test]
#[serial]
async fn create_tenant_provisions_only_its_own_slug() {
    let Some(store) = fresh().await else {
        eprintln!("skip: QUARK_TEST_DATABASE_URL not set");
        return;
    };
    let store = Arc::new(store);
    let (_x_user, raw_x) = seed_session(&store, "kc-sweep-x-subject").await;
    let (_y_user, raw_y) = seed_session(&store, "kc-sweep-y-subject").await;

    let mock = Arc::new(quark::keycloak::testing::MockKeycloakAdmin::default());
    let app = app_over_with_keycloak(
        store.clone() as Arc<dyn Store>,
        store.clone() as Arc<dyn AnalyticsSink>,
        true,
        Some(mock.clone() as Arc<dyn quark::keycloak::KeycloakAdmin>),
        Some("https://kc.example.com".to_string()),
    );

    for (raw, name, slug) in [
        (&raw_x, "Sweep X", "kc-sweep-x"),
        (&raw_y, "Sweep Y", "kc-sweep-y"),
    ] {
        let resp = app
            .clone()
            .oneshot(
                Request::post("/admin/tenants")
                    .header("content-type", "application/json")
                    .header("cookie", format!("qk_session={raw}"))
                    .body(Body::from(
                        serde_json::json!({"name": name, "slug": slug}).to_string(),
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
    }

    let calls = mock.calls();
    assert!(
        calls.contains(&"ensure_realm(kc-sweep-x)".to_string()),
        "tenant X's own slug must be provisioned"
    );
    assert!(
        calls.contains(&"ensure_realm(kc-sweep-y)".to_string()),
        "tenant Y's own slug must be provisioned"
    );
    for call in &calls {
        let has_x = call.contains("kc-sweep-x");
        let has_y = call.contains("kc-sweep-y");
        assert!(
            !(has_x && has_y),
            "a single provisioning call must never reference both tenants' slugs: {call}"
        );
    }
}

/// Best-effort provisioning: a `KeycloakAdmin` whose `ensure_realm` always
/// errors must not stop tenant creation. The tenant and its Owner membership
/// are already committed by the time provisioning runs, so the response
/// (and the created tenant/membership) must be exactly as if Keycloak had
/// never been configured — the boot backfill is what retries it.
struct EnsureRealmFailsKeycloakAdmin;

#[async_trait::async_trait]
impl quark::keycloak::KeycloakAdmin for EnsureRealmFailsKeycloakAdmin {
    async fn ensure_realm(&self, _slug: &str) -> Result<(), quark::keycloak::KcError> {
        Err(quark::keycloak::KcError("ensure_realm unavailable".into()))
    }
    async fn ensure_client(
        &self,
        _slug: &str,
        _redirect_uri: &str,
    ) -> Result<(), quark::keycloak::KcError> {
        unreachable!("provisioning must stop at ensure_realm's failure")
    }
    async fn ensure_groups_and_mapper(&self, _slug: &str) -> Result<(), quark::keycloak::KcError> {
        unreachable!("provisioning must stop at ensure_realm's failure")
    }
    async fn ensure_user(
        &self,
        _slug: &str,
        _email: &str,
        _group: &str,
    ) -> Result<String, quark::keycloak::KcError> {
        unreachable!("provisioning must stop at ensure_realm's failure")
    }
    async fn send_set_password_email(
        &self,
        _slug: &str,
        _user_id: &str,
    ) -> Result<(), quark::keycloak::KcError> {
        unreachable!("provisioning must stop at ensure_realm's failure")
    }
}

#[tokio::test]
#[serial]
async fn create_tenant_survives_ensure_realm_failure() {
    let Some(store) = fresh().await else {
        eprintln!("skip: QUARK_TEST_DATABASE_URL not set");
        return;
    };
    let store = Arc::new(store);
    let (user_id, raw) = seed_session(&store, "kc-realm-fail-subject").await;

    let app = app_over_with_keycloak(
        store.clone() as Arc<dyn Store>,
        store.clone() as Arc<dyn AnalyticsSink>,
        true,
        Some(Arc::new(EnsureRealmFailsKeycloakAdmin) as Arc<dyn quark::keycloak::KeycloakAdmin>),
        Some("https://kc.example.com".to_string()),
    );

    let resp = app
        .oneshot(
            Request::post("/admin/tenants")
                .header("content-type", "application/json")
                .header("cookie", format!("qk_session={raw}"))
                .body(Body::from(
                    r#"{"name":"Realm Fail Co","slug":"kc-realm-fail-co"}"#,
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(
        resp.status(),
        StatusCode::OK,
        "a Keycloak provisioning failure must never fail tenant creation"
    );
    let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .unwrap();
    let tenant: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    let tenant_id = tenant["id"].as_u64().unwrap();

    let membership = store
        .get_membership(user_id, TenantId(tenant_id))
        .await
        .unwrap()
        .expect("the Owner membership must still be committed");
    assert_eq!(membership.role, quark::tenant::Role::Owner);
    assert!(
        store
            .get_oidc_config_bare(TenantId(tenant_id))
            .await
            .unwrap()
            .is_none(),
        "with ensure_realm failing, no oidc_config must be written (the backfill retries it)"
    );
}
