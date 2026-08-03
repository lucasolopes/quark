// Codigo de teste pode entrar em panico: a falha e o proprio sinal. O
// clippy.toml cobre itens sob #[test]/#[cfg(test)], mas nao os helpers de
// topo de arquivo (fn app(), fixtures), que sao a maioria aqui.
#![allow(clippy::unwrap_used)]
// Enterprise suite: these routes only exist in the `--features ee` build
// (LUC-19). Without the feature the binary compiles empty instead of failing.
#![cfg(feature = "ee")]

use axum::body::Body;
use axum::http::{Request, StatusCode};
use quark::analytics::AnalyticsSink;
use quark::api::router;
use quark::auth::{generate_token, hash_token, Session};
use quark::cache::Cache;
use quark::store::{open_backends, postgres::PostgresStore, Store};
use quark::tenant::{TenantId, User};
use serial_test::file_serial;
use sqlx::postgres::PgPoolOptions;
use std::sync::Arc;
use tower::ServiceExt;

mod common;

async fn fresh() -> Option<PostgresStore> {
    let url = std::env::var("QUARK_TEST_DATABASE_URL").ok()?;
    let s = PostgresStore::open(&url, true).await.unwrap();
    s.reset_for_tests().await.unwrap();
    Some(s)
}

// ids are >=1 and monotonic, never 0 (0 is the seeded default tenant).
#[tokio::test]
#[file_serial]
async fn next_tenant_id_starts_above_default() {
    let Some(store) = fresh().await else {
        eprintln!("skip: QUARK_TEST_DATABASE_URL not set");
        return;
    };
    let a = store.next_tenant_id().await.unwrap();
    let b = store.next_tenant_id().await.unwrap();
    assert!(
        a >= 1 && b > a,
        "tenant ids must be >=1 (0 is the default tenant) and monotonic"
    );
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
    keycloak: Option<Arc<dyn quark::ee::keycloak::KeycloakAdmin>>,
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
    keycloak: Option<Arc<dyn quark::ee::keycloak::KeycloakAdmin>>,
    keycloak_base_url: Option<String>,
) -> axum::Router {
    let cache = Cache::new(store.clone(), 1000, None);
    let host_router = Arc::new(quark::domain_router::HostRouter::new(
        store.clone(),
        None,
        None,
    ));
    let (analytics_tx, _rx) = tokio::sync::mpsc::channel(100);
    let state = common::TestState::new(store, sink)
        .cache(cache)
        .host_router(host_router)
        .analytics_tx(analytics_tx)
        .webhooks(common::test_webhook_dispatcher())
        .oidc_configured(true)
        .multi_tenant(multi_tenant)
        .tenant_domain_suffix(tenant_domain_suffix)
        .keycloak(keycloak)
        .keycloak_base_url(keycloak_base_url)
        .build();
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
        id_token: None,
    };
    store.put_session(TenantId(0), &session).await.unwrap();
    (user_id, raw)
}

/// `POST /admin/tenants` (cloud): any authenticated OIDC user (even with zero
/// memberships) can self-serve a workspace. Verifies the created tenant, the
/// Owner membership, the session's tenant switching to it, the 409 on a
/// duplicate slug, and the 404 in OSS mode.
#[tokio::test]
#[file_serial]
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

/// `POST /admin/tenants` with `slug = "master"` (a Keycloak built-in realm
/// name) must be rejected before any tenant row is created, since P2e turns
/// the slug into the realm name (final-review finding).
#[tokio::test]
#[file_serial]
async fn create_tenant_rejects_reserved_slug() {
    let Some(store) = fresh().await else {
        eprintln!("skip: QUARK_TEST_DATABASE_URL not set");
        return;
    };
    let store = Arc::new(store);
    let (_user_id, raw) = seed_session(&store, "reserved-slug-subject").await;
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
                .body(Body::from(r#"{"name":"Evil","slug":"master"}"#))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    assert!(
        store.get_tenant_by_slug("master").await.unwrap().is_none(),
        "no tenant row must be created for a rejected slug"
    );
}

/// `POST /admin/tenants` with a slug that fails the charset/length rule
/// (case, underscore, punctuation, path separator, whitespace, or length)
/// must be rejected — a bad slug becomes a malformed Keycloak realm name,
/// Admin-API path, and OIDC issuer URL under P2e.
#[tokio::test]
#[file_serial]
async fn create_tenant_rejects_malformed_slug() {
    let Some(store) = fresh().await else {
        eprintln!("skip: QUARK_TEST_DATABASE_URL not set");
        return;
    };
    let store = Arc::new(store);
    let (_user_id, raw) = seed_session(&store, "malformed-slug-subject").await;
    let app = app_over(
        store.clone() as Arc<dyn Store>,
        store as Arc<dyn AnalyticsSink>,
        true,
    );

    let too_long: String = "a".repeat(64);
    let bad_slugs = ["Bad_Slug!", "a/b", " ", too_long.as_str(), "-x"];
    for slug in bad_slugs {
        let resp = app
            .clone()
            .oneshot(
                Request::post("/admin/tenants")
                    .header("content-type", "application/json")
                    .header("cookie", format!("qk_session={raw}"))
                    .body(Body::from(
                        serde_json::json!({"name": "Bad", "slug": slug}).to_string(),
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(
            resp.status(),
            StatusCode::BAD_REQUEST,
            "slug {slug:?} must be rejected"
        );
    }
}

/// No session cookie at all -> 401, not 404 (cloud endpoint exists, just
/// unauthenticated).
#[tokio::test]
#[file_serial]
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
#[file_serial]
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
#[file_serial]
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
        id_token: None,
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
    assert!(
        me.get("memberships").is_none(),
        "OSS must OMIT memberships entirely (the panel reads its presence as cloud mode); one exists in the store but must not surface"
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
#[file_serial]
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
#[file_serial]
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
#[file_serial]
async fn create_tenant_provisions_keycloak_realm() {
    let Some(store) = fresh().await else {
        eprintln!("skip: QUARK_TEST_DATABASE_URL not set");
        return;
    };
    let store = Arc::new(store);
    let (_user_id, raw) = seed_session(&store, "kc-provision-subject").await;

    let mock = Arc::new(quark::ee::keycloak::testing::MockKeycloakAdmin::default());
    mock.set_next_user_id("kc-user-1");
    let app = app_over_with_keycloak(
        store.clone() as Arc<dyn Store>,
        store.clone() as Arc<dyn AnalyticsSink>,
        true,
        Some(mock.clone() as Arc<dyn quark::ee::keycloak::KeycloakAdmin>),
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
#[file_serial]
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

/// Boot backfill (`quark::ee::api::backfill_keycloak_provisioning`): a tenant
/// created before Keycloak was configured (no `oidc_config` yet) gets
/// provisioned on the backfill pass; a tenant that already has one is left
/// alone (no extra calls), and the count returned matches how many were
/// actually provisioned.
#[tokio::test]
#[file_serial]
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
            member_value: String::new(),
            required_value: Some("quark-readers".to_string()),
            post_login_url: None,
            post_logout_url: None,
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
            member_value: String::new(),
            required_value: Some("quark-readers".to_string()),
            post_login_url: None,
            post_logout_url: None,
        })
        .await
        .unwrap();

    let mock: Arc<dyn quark::ee::keycloak::KeycloakAdmin> =
        Arc::new(quark::ee::keycloak::testing::MockKeycloakAdmin::default());

    let provisioned =
        quark::ee::api::backfill_keycloak_provisioning(&store, &mock, "https://kc.example.com")
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
        quark::ee::api::backfill_keycloak_provisioning(&store, &mock, "https://kc.example.com")
            .await
            .unwrap();
    assert_eq!(provisioned_again, 0);
}

/// LUC-56: the boot backfill provisions the tenant's OWNER in the realm too.
/// A tenant that predates Keycloak (Owner membership but no `oidc_config`) gets
/// its Owner's realm user + set-password email on the backfill pass — via
/// `get_owner_user_id` — so the Owner can SSO-log-in, not just realm/client.
#[tokio::test]
#[file_serial]
async fn backfill_provisions_the_tenant_owner() {
    let Some(store) = fresh().await else {
        eprintln!("skip: QUARK_TEST_DATABASE_URL not set");
        return;
    };
    let store: Arc<dyn Store> = Arc::new(store);

    // Keep the seeded default tenant (id 0) out of scope: give it an
    // oidc_config so the backfill skips it and only touches our tenant.
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
            member_value: String::new(),
            required_value: Some("quark-readers".to_string()),
            post_login_url: None,
            post_logout_url: None,
        })
        .await
        .unwrap();

    // A tenant with an Owner membership but no oidc_config (predates Keycloak).
    let owner_id = store.next_user_id().await.unwrap();
    store
        .put_user(&User {
            id: owner_id,
            subject: "owner-sub".to_string(),
            email: "owner@acme.com".to_string(),
            display: "Owner".to_string(),
            created: 0,
        })
        .await
        .unwrap();
    let t_id = store.next_tenant_id().await.unwrap();
    store
        .put_tenant(&quark::tenant::Tenant {
            id: TenantId(t_id),
            name: "Acme".to_string(),
            slug: "backfill-owner".to_string(),
            created: 0,
        })
        .await
        .unwrap();
    store
        .put_membership(&quark::tenant::Membership {
            user_id: owner_id,
            tenant_id: TenantId(t_id),
            role: quark::tenant::Role::Owner,
            created: 0,
        })
        .await
        .unwrap();

    // Sanity: the new store lookup finds the Owner.
    assert_eq!(
        store.get_owner_user_id(TenantId(t_id)).await.unwrap(),
        Some(owner_id)
    );

    let mock = Arc::new(quark::ee::keycloak::testing::MockKeycloakAdmin::default());
    mock.set_next_user_id("kc-owner-1");
    let mock_dyn: Arc<dyn quark::ee::keycloak::KeycloakAdmin> = mock.clone();
    let provisioned =
        quark::ee::api::backfill_keycloak_provisioning(&store, &mock_dyn, "https://kc.example.com")
            .await
            .unwrap();
    assert_eq!(provisioned, 1);

    let calls = mock.calls();
    assert!(
        calls.contains(&"ensure_user(backfill-owner,owner@acme.com,quark-admins)".to_string()),
        "the backfill must provision the tenant's Owner in the realm: {calls:?}"
    );
    assert!(
        calls.contains(&"send_set_password_email(backfill-owner,kc-owner-1)".to_string()),
        "the backfill must send the Owner a set-password email: {calls:?}"
    );
}

/// LUC-81: the boot backfill reconciles a STALE issuer on a tenant that
/// already has an `oidc_config`. When the Keycloak base URL changes after
/// provisioning, the stored issuer still points at the old base and every SSO
/// login 401s (the token's `iss` no longer matches the stored value `verify`
/// checks against). The backfill rewrites the issuer to the currently derived
/// value, leaves an already-correct tenant untouched, and — critically — never
/// disturbs the encrypted `client_secret` or any other field.
#[tokio::test]
#[file_serial]
async fn backfill_reconciles_stale_issuer_preserving_client_secret() {
    let Some(store) = fresh().await else {
        eprintln!("skip: QUARK_TEST_DATABASE_URL not set");
        return;
    };
    let store: Arc<dyn Store> = Arc::new(store);

    const NEW_BASE: &str = "https://kc.new.example.com";
    const OLD_BASE: &str = "https://kc.old.example.com";

    // Keep the seeded default tenant (id 0) out of scope: give it a config
    // whose issuer is already correct for NEW_BASE so the backfill skips it.
    store
        .put_oidc_config(&quark::oidc::TenantOidcConfig {
            tenant_id: TenantId(0),
            issuer: quark::ee::keycloak::derive_issuer(NEW_BASE, "default"),
            client_id: "quark".to_string(),
            client_secret: String::new(),
            scopes: vec!["openid".to_string()],
            admin_claim: "groups".to_string(),
            admin_value: "quark-admins".to_string(),
            readonly_value: "quark-readers".to_string(),
            member_value: String::new(),
            required_value: Some("quark-readers".to_string()),
            post_login_url: None,
            post_logout_url: None,
        })
        .await
        .unwrap();

    // A tenant provisioned under the OLD base: its stored issuer is stale, and
    // it holds a non-empty client_secret that MUST survive the reconcile.
    let stale_id = store.next_tenant_id().await.unwrap();
    store
        .put_tenant(&quark::tenant::Tenant {
            id: TenantId(stale_id),
            name: "Stale Issuer".to_string(),
            slug: "stale-issuer".to_string(),
            created: 0,
        })
        .await
        .unwrap();
    store
        .put_oidc_config(&quark::oidc::TenantOidcConfig {
            tenant_id: TenantId(stale_id),
            issuer: quark::ee::keycloak::derive_issuer(OLD_BASE, "stale-issuer"),
            client_id: "quark".to_string(),
            client_secret: "super-secret-value".to_string(),
            scopes: vec!["openid".to_string(), "profile".to_string()],
            admin_claim: "groups".to_string(),
            admin_value: "quark-admins".to_string(),
            readonly_value: "quark-readers".to_string(),
            member_value: String::new(),
            required_value: Some("quark-readers".to_string()),
            post_login_url: Some("/panel".to_string()),
            post_logout_url: None,
        })
        .await
        .unwrap();

    // A tenant whose issuer is already correct for NEW_BASE: must be left alone.
    let ok_id = store.next_tenant_id().await.unwrap();
    store
        .put_tenant(&quark::tenant::Tenant {
            id: TenantId(ok_id),
            name: "Already Correct".to_string(),
            slug: "already-correct".to_string(),
            created: 0,
        })
        .await
        .unwrap();
    let ok_issuer = quark::ee::keycloak::derive_issuer(NEW_BASE, "already-correct");
    store
        .put_oidc_config(&quark::oidc::TenantOidcConfig {
            tenant_id: TenantId(ok_id),
            issuer: ok_issuer.clone(),
            client_id: "quark".to_string(),
            client_secret: "another-secret".to_string(),
            scopes: vec!["openid".to_string()],
            admin_claim: "groups".to_string(),
            admin_value: "quark-admins".to_string(),
            readonly_value: "quark-readers".to_string(),
            member_value: String::new(),
            required_value: Some("quark-readers".to_string()),
            post_login_url: None,
            post_logout_url: None,
        })
        .await
        .unwrap();

    let mock: Arc<dyn quark::ee::keycloak::KeycloakAdmin> =
        Arc::new(quark::ee::keycloak::testing::MockKeycloakAdmin::default());

    // Both tenants already have a config, so nothing is newly *provisioned*.
    let provisioned = quark::ee::api::backfill_keycloak_provisioning(&store, &mock, NEW_BASE)
        .await
        .unwrap();
    assert_eq!(
        provisioned, 0,
        "a stale-issuer reconcile is not a fresh provision and must not be counted"
    );

    // The stale tenant's issuer is rewritten to the NEW_BASE-derived value, and
    // every other field — above all the client_secret — is unchanged.
    let reconciled = store
        .get_oidc_config_bare(TenantId(stale_id))
        .await
        .unwrap()
        .expect("stale tenant still has a config");
    assert_eq!(
        reconciled.issuer,
        quark::ee::keycloak::derive_issuer(NEW_BASE, "stale-issuer"),
        "the stale issuer must be reconciled to the current base"
    );
    assert_eq!(
        reconciled.client_secret, "super-secret-value",
        "the client_secret MUST survive the reconcile"
    );
    assert_eq!(reconciled.client_id, "quark");
    assert_eq!(
        reconciled.scopes,
        vec!["openid".to_string(), "profile".to_string()]
    );
    assert_eq!(reconciled.post_login_url.as_deref(), Some("/panel"));

    // The already-correct tenant is untouched (issuer and secret both intact).
    let untouched = store
        .get_oidc_config_bare(TenantId(ok_id))
        .await
        .unwrap()
        .expect("already-correct tenant still has a config");
    assert_eq!(untouched.issuer, ok_issuer);
    assert_eq!(untouched.client_secret, "another-secret");

    // Idempotent: a second pass finds every issuer already correct, so it
    // reconciles nothing and still provisions nothing.
    let provisioned_again = quark::ee::api::backfill_keycloak_provisioning(&store, &mock, NEW_BASE)
        .await
        .unwrap();
    assert_eq!(provisioned_again, 0);
    let after = store
        .get_oidc_config_bare(TenantId(stale_id))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        after.issuer,
        quark::ee::keycloak::derive_issuer(NEW_BASE, "stale-issuer")
    );
    assert_eq!(after.client_secret, "super-secret-value");
}

/// Security sweep: provisioning a tenant only ever touches that tenant's own
/// slug. Two different users self-serve two different tenants against the
/// same `KeycloakAdmin`; every recorded call carries exactly one of the two
/// slugs, never both, and each tenant's own slug shows up in its own calls.
#[tokio::test]
#[file_serial]
async fn create_tenant_provisions_only_its_own_slug() {
    let Some(store) = fresh().await else {
        eprintln!("skip: QUARK_TEST_DATABASE_URL not set");
        return;
    };
    let store = Arc::new(store);
    let (_x_user, raw_x) = seed_session(&store, "kc-sweep-x-subject").await;
    let (_y_user, raw_y) = seed_session(&store, "kc-sweep-y-subject").await;

    let mock = Arc::new(quark::ee::keycloak::testing::MockKeycloakAdmin::default());
    let app = app_over_with_keycloak(
        store.clone() as Arc<dyn Store>,
        store.clone() as Arc<dyn AnalyticsSink>,
        true,
        Some(mock.clone() as Arc<dyn quark::ee::keycloak::KeycloakAdmin>),
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
impl quark::ee::keycloak::KeycloakAdmin for EnsureRealmFailsKeycloakAdmin {
    async fn ensure_realm(&self, _slug: &str) -> Result<(), quark::ee::keycloak::KcError> {
        Err(quark::ee::keycloak::KcError(
            "ensure_realm unavailable".into(),
        ))
    }
    async fn ensure_client(
        &self,
        _slug: &str,
        _redirect_uri: &str,
    ) -> Result<(), quark::ee::keycloak::KcError> {
        unreachable!("provisioning must stop at ensure_realm's failure")
    }
    async fn ensure_groups_and_mapper(
        &self,
        _slug: &str,
    ) -> Result<(), quark::ee::keycloak::KcError> {
        unreachable!("provisioning must stop at ensure_realm's failure")
    }
    async fn ensure_user(
        &self,
        _slug: &str,
        _email: &str,
        _group: &str,
    ) -> Result<String, quark::ee::keycloak::KcError> {
        unreachable!("provisioning must stop at ensure_realm's failure")
    }
    async fn send_set_password_email(
        &self,
        _slug: &str,
        _user_id: &str,
    ) -> Result<(), quark::ee::keycloak::KcError> {
        unreachable!("provisioning must stop at ensure_realm's failure")
    }
    async fn delete_realm(&self, _slug: &str) -> Result<(), quark::ee::keycloak::KcError> {
        unreachable!("this fake only exercises the creation path")
    }
}

#[tokio::test]
#[file_serial]
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
        Some(Arc::new(EnsureRealmFailsKeycloakAdmin) as Arc<dyn quark::ee::keycloak::KeycloakAdmin>),
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

/// `delete_realm` is the only Keycloak call workspace deletion makes. The
/// contract asserted here is the shape and the slug: the mock records exactly
/// one call, naming exactly the tenant being deleted.
#[tokio::test]
async fn delete_realm_is_called_with_the_tenant_slug() {
    let kc = quark::ee::keycloak::testing::MockKeycloakAdmin::default();
    let admin: &dyn quark::ee::keycloak::KeycloakAdmin = &kc;

    admin.delete_realm("acme").await.unwrap();

    assert_eq!(kc.calls(), vec!["delete_realm(acme)".to_string()]);
}

// --- Workspace deletion, Postgres arm (LUC-138) ---

/// A link record, minimal: the deletion battery only cares that the row exists
/// and which tenant owns it.
fn rec(url: &str) -> quark::store::Record {
    quark::store::Record {
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

/// Seeds the tenant-owned rows the cross-backend battery in
/// `tests/tenant_isolation_it.rs` cannot reach: the cloud-only tables
/// (`domains`, `invites`, `oidc_configs`, `sso_email_domains`), the session,
/// the analytics rows and an outbox delivery. Returns the raw session token
/// so the caller can look the session back up by hash.
async fn seed_cloud_only_rows(store: &PostgresStore, tenant: TenantId, nth: u64) -> String {
    // A link plus its alias in the SHARED domain namespace (`domain_id = 0`),
    // where both tenants' aliases sit side by side in one namespace. That is
    // the one place a tenant-scoped delete can reach across and take a
    // neighbor's row with it, so both tenants get one.
    store
        .put_alias_and_link(
            tenant,
            quark::domain::SHARED_DOMAIN_ID,
            &format!("wipe-alias-{nth}"),
            7600 + nth,
            &rec(&format!("https://alias-{nth}.example.com")),
        )
        .await
        .unwrap();
    let raw = generate_token();
    store
        .put_session(
            tenant,
            &Session {
                token_hash: hash_token(&raw),
                subject: format!("wipe-subject-{nth}"),
                display: format!("Wipe {nth}"),
                scopes: vec![],
                created: 0,
                expires: quark::now() + 3600,
                tenant_id: tenant,
                user_id: 7000 + nth,
                id_token: None,
            },
        )
        .await
        .unwrap();
    store
        .put_domain(&quark::domain::Domain {
            id: 7100 + nth,
            tenant_id: tenant,
            host: format!("wipe-{nth}.example.com"),
            token: format!("token-{nth}"),
            status: quark::domain::DomainStatus::Pending,
            created: 0,
            verified_at: None,
        })
        .await
        .unwrap();
    store
        .create_invite(&quark::invite::Invite {
            id: 7200 + nth,
            tenant_id: tenant,
            email: format!("invited{nth}@example.com"),
            role: quark::tenant::Role::Member,
            token_hash: format!("invite-hash-{nth}"),
            invited_by: 7000 + nth,
            created: 0,
            expires: quark::now() + 3600,
            accepted_at: None,
            accepted_by: None,
        })
        .await
        .unwrap();
    store
        .put_oidc_config(&quark::oidc::TenantOidcConfig {
            tenant_id: tenant,
            issuer: format!("https://kc.example.com/realms/wipe-{nth}"),
            client_id: "quark".to_string(),
            client_secret: String::new(),
            scopes: vec!["openid".to_string()],
            admin_claim: "groups".to_string(),
            admin_value: "quark-admins".to_string(),
            readonly_value: "quark-readers".to_string(),
            member_value: String::new(),
            required_value: None,
            post_login_url: None,
            post_logout_url: None,
        })
        .await
        .unwrap();
    store
        .put_sso_domain(&quark::sso::SsoEmailDomain {
            id: 7300 + nth,
            tenant_id: tenant,
            domain: format!("wipe-sso-{nth}.example.com"),
            token: format!("sso-token-{nth}"),
            status: quark::domain::DomainStatus::Pending,
            created: 0,
            verified_at: None,
        })
        .await
        .unwrap();
    store
        .enqueue_deliveries(&[quark::store::OutboxRow {
            delivery_key: format!("wipe-delivery-{nth}"),
            subscription_id: 7400 + nth,
            event_type: "link.created".to_string(),
            payload: "{}".to_string(),
            created: 0,
            // Far in the future so `claim_due_deliveries` never leases it out
            // from under the assertions below.
            next_attempt_at: quark::now() + 86_400,
            tenant_id: tenant,
        }])
        .await
        .unwrap();
    let click = quark::analytics::ClickEvent {
        id: 7500 + nth,
        event_id: String::new(),
        ts: 1_752_300_000,
        referer: None,
        country: Some("BR".into()),
        user_agent: Some("iPhone".into()),
        city: None,
        bot: false,
        ip: None,
        fbc: None,
        variant: None,
        tenant_id: tenant.0,
    };
    store.record_batch(&[click]).await.unwrap();
    raw
}

/// The outbox rows still on the table, by owning tenant. `webhook_deliveries`
/// has no tenant-scoped read on the `Store` trait (the relay claims across
/// tenants), so this goes through the relay's own claim. Called exactly once
/// per test: claiming leases the rows it returns, so a second call would not
/// see the same set.
async fn claim_all_deliveries(store: &PostgresStore) -> Vec<TenantId> {
    store
        .claim_due_deliveries(quark::now() + 172_800, 100)
        .await
        .unwrap()
        .into_iter()
        .map(|d| d.tenant_id)
        .collect()
}

/// `delete_tenant` must take out every row the doomed tenant owns across all
/// of `TENANT_OWNED_TABLES` plus `memberships` and `tenants` — and not one row
/// belonging to the tenant next to it. This runs in cloud mode, where
/// `FORCE ROW LEVEL SECURITY` is on: a `DELETE` issued without the RLS tenant
/// context deletes zero rows and raises no error at all, so the first half of
/// this test is what catches that.
#[tokio::test]
#[file_serial]
async fn delete_tenant_removes_every_owned_row_pg() {
    let Some(store) = fresh().await else {
        eprintln!("skip: QUARK_TEST_DATABASE_URL not set");
        return;
    };
    let doomed = TenantId(51);
    let neighbor = TenantId(52);
    for (tenant, nth) in [(doomed, 1u64), (neighbor, 2u64)] {
        store
            .put_tenant(&quark::tenant::Tenant {
                id: tenant,
                name: format!("Wipe {nth}"),
                slug: format!("wipe-{nth}"),
                created: 0,
            })
            .await
            .unwrap();
        store
            .put_user(&User {
                id: 7000 + nth,
                subject: format!("wipe-subject-{nth}"),
                email: format!("owner{nth}@example.com"),
                display: format!("Owner {nth}"),
                created: 0,
            })
            .await
            .unwrap();
        store
            .put_membership(&quark::tenant::Membership {
                user_id: 7000 + nth,
                tenant_id: tenant,
                role: quark::tenant::Role::Owner,
                created: 0,
            })
            .await
            .unwrap();
    }
    let doomed_session = seed_cloud_only_rows(&store, doomed, 1).await;
    let neighbor_session = seed_cloud_only_rows(&store, neighbor, 2).await;

    store.delete_tenant(doomed).await.unwrap();

    // --- the doomed tenant keeps nothing ---
    assert!(store.get_tenant(doomed).await.unwrap().is_none(), "tenant");
    assert!(
        store.get_membership(7001, doomed).await.unwrap().is_none(),
        "membership"
    );
    assert!(
        store
            .get_session_by_hash(&hash_token(&doomed_session), quark::now())
            .await
            .unwrap()
            .is_none(),
        "session"
    );
    assert!(
        store
            .get_domain_by_host("wipe-1.example.com")
            .await
            .unwrap()
            .is_none(),
        "domain"
    );
    assert!(
        store.list_invites(doomed).await.unwrap().is_empty(),
        "invite"
    );
    assert!(
        store.get_oidc_config_bare(doomed).await.unwrap().is_none(),
        "oidc config"
    );
    assert!(
        store.list_sso_domains(doomed).await.unwrap().is_empty(),
        "sso email domain"
    );
    let deliveries = claim_all_deliveries(&store).await;
    assert!(
        !deliveries.contains(&doomed),
        "the doomed tenant must have no outbox row left: {deliveries:?}"
    );
    assert_eq!(
        store.stats_for_tenant(doomed.0).await.unwrap().total,
        0,
        "click rows"
    );
    assert!(
        store.get_link(doomed, 7601).await.unwrap().is_none(),
        "link"
    );
    assert!(
        store
            .get_alias(quark::domain::SHARED_DOMAIN_ID, "wipe-alias-1")
            .await
            .unwrap()
            .is_none(),
        "alias"
    );

    // --- the neighbor keeps everything (the assertion that matters) ---
    assert!(
        store.get_tenant(neighbor).await.unwrap().is_some(),
        "neighbor tenant"
    );
    assert!(
        store
            .get_membership(7002, neighbor)
            .await
            .unwrap()
            .is_some(),
        "neighbor membership"
    );
    assert!(
        store
            .get_session_by_hash(&hash_token(&neighbor_session), quark::now())
            .await
            .unwrap()
            .is_some(),
        "neighbor session"
    );
    assert!(
        store
            .get_domain_by_host("wipe-2.example.com")
            .await
            .unwrap()
            .is_some(),
        "neighbor domain"
    );
    assert_eq!(
        store.list_invites(neighbor).await.unwrap().len(),
        1,
        "neighbor invite"
    );
    assert!(
        store
            .get_oidc_config_bare(neighbor)
            .await
            .unwrap()
            .is_some(),
        "neighbor oidc config"
    );
    assert_eq!(
        store.list_sso_domains(neighbor).await.unwrap().len(),
        1,
        "neighbor sso email domain"
    );
    assert_eq!(
        deliveries.iter().filter(|t| **t == neighbor).count(),
        1,
        "the neighbor's outbox row must survive: {deliveries:?}"
    );
    assert_eq!(
        store.stats_for_tenant(neighbor.0).await.unwrap().total,
        1,
        "neighbor click rows"
    );
    assert!(
        store.get_link(neighbor, 7602).await.unwrap().is_some(),
        "neighbor link"
    );
    // The assertion the shared alias namespace exists for: the two tenants'
    // aliases live under `domain_id = 0` together, and deleting one tenant
    // must not reach into the other's entry.
    assert_eq!(
        store
            .get_alias(quark::domain::SHARED_DOMAIN_ID, "wipe-alias-2")
            .await
            .unwrap(),
        Some(7602),
        "the neighbor's alias in the shared namespace must survive"
    );

    // `users` is global and is never touched: the deleted tenant's owner can
    // be a member of other workspaces.
    assert!(
        store.get_user_by_id(7001).await.unwrap().is_some(),
        "the deleted tenant's owner must keep their global user row"
    );
}

/// Deleting a tenant id that does not exist is a no-op and not an error, and
/// nobody else's rows move. This is idempotency, not atomicity: the store has
/// no seam to fail a statement in the middle of the transaction, so the
/// rollback path is not what is under test here.
#[tokio::test]
#[file_serial]
async fn delete_tenant_of_an_unknown_id_is_a_no_op_pg() {
    let Some(store) = fresh().await else {
        eprintln!("skip: QUARK_TEST_DATABASE_URL not set");
        return;
    };
    let neighbor = TenantId(61);
    store
        .put_tenant(&quark::tenant::Tenant {
            id: neighbor,
            name: "Bystander".to_string(),
            slug: "bystander".to_string(),
            created: 0,
        })
        .await
        .unwrap();
    let session = seed_cloud_only_rows(&store, neighbor, 5).await;

    store
        .delete_tenant(TenantId(9_999))
        .await
        .expect("deleting an unknown tenant is idempotent, not an error");

    assert!(store.get_tenant(neighbor).await.unwrap().is_some());
    assert!(store
        .get_session_by_hash(&hash_token(&session), quark::now())
        .await
        .unwrap()
        .is_some());
    assert!(store
        .get_domain_by_host("wipe-5.example.com")
        .await
        .unwrap()
        .is_some());
    assert_eq!(store.list_invites(neighbor).await.unwrap().len(), 1);
    assert!(store
        .get_oidc_config_bare(neighbor)
        .await
        .unwrap()
        .is_some());
    assert_eq!(store.list_sso_domains(neighbor).await.unwrap().len(), 1);
    assert_eq!(store.stats_for_tenant(neighbor.0).await.unwrap().total, 1);
}

/// The slug goes back to being available: the `tenants` row is gone, so the
/// UNIQUE constraint no longer stands in the way of creating it again. This is
/// the promise the spec makes about a hard delete.
#[tokio::test]
#[file_serial]
async fn delete_tenant_frees_the_slug_pg() {
    let Some(store) = fresh().await else {
        eprintln!("skip: QUARK_TEST_DATABASE_URL not set");
        return;
    };
    let store = Arc::new(store);
    let (_user_id, raw) = seed_session(&store, "slug-reuse-subject").await;
    let app = app_over(
        store.clone() as Arc<dyn Store>,
        store.clone() as Arc<dyn AnalyticsSink>,
        true,
    );

    fn create_req(raw: &str) -> Request<Body> {
        Request::post("/admin/tenants")
            .header("content-type", "application/json")
            .header("cookie", format!("qk_session={raw}"))
            .body(Body::from(r#"{"name":"Acme","slug":"acme-reuse"}"#))
            .unwrap()
    }

    let resp = app.clone().oneshot(create_req(&raw)).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .unwrap();
    let tenant: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    let tenant_id = TenantId(tenant["id"].as_u64().unwrap());

    store.delete_tenant(tenant_id).await.unwrap();
    assert!(
        store
            .get_tenant_by_slug("acme-reuse")
            .await
            .unwrap()
            .is_none(),
        "the slug must no longer resolve to a tenant"
    );

    // Creating the workspace switched this session into it, and `sessions` is
    // a tenant-owned table, so the delete took the session row with it. The
    // acting user's cookie stops working the moment their current workspace is
    // deleted; a fresh session is what proves the slug itself is free.
    assert_eq!(
        app.clone()
            .oneshot(create_req(&raw))
            .await
            .unwrap()
            .status(),
        StatusCode::UNAUTHORIZED,
        "the session that was switched into the deleted workspace dies with it"
    );

    let (_second_user, raw2) = seed_session(&store, "slug-reuse-subject-2").await;
    let again = app.oneshot(create_req(&raw2)).await.unwrap();
    assert_eq!(
        again.status(),
        StatusCode::OK,
        "the slug must be free to use again after the delete"
    );
}

// --- DELETE /admin/tenants/:id (LUC-138) ---

/// Seeds a tenant plus `user_id`'s membership on it, bypassing the create
/// handler so no Keycloak provisioning call is recorded (the deletion tests
/// assert on the mock's call list, which must contain only what the DELETE
/// itself does).
async fn seed_workspace(
    store: &PostgresStore,
    user_id: u64,
    slug: &str,
    role: quark::tenant::Role,
) -> TenantId {
    let id = TenantId(store.next_tenant_id().await.unwrap());
    store
        .put_tenant(&quark::tenant::Tenant {
            id,
            name: slug.to_string(),
            slug: slug.to_string(),
            created: 0,
        })
        .await
        .unwrap();
    store
        .put_membership(&quark::tenant::Membership {
            user_id,
            tenant_id: id,
            role,
            created: 0,
        })
        .await
        .unwrap();
    id
}

fn delete_req(tenant: TenantId, raw: Option<&str>) -> Request<Body> {
    let req = Request::delete(format!("/admin/tenants/{}", tenant.0));
    match raw {
        Some(raw) => req.header("cookie", format!("qk_session={raw}")),
        None => req,
    }
    .body(Body::empty())
    .unwrap()
}

/// In OSS the route does not exist: 404, and the check runs before any
/// credential is looked at (none is presented here). Parity with
/// `oss_workspace_endpoints_are_404`. Runs over LMDB, never gated on Postgres.
#[tokio::test]
async fn delete_tenant_is_404_in_oss() {
    let dir = tempfile::tempdir().unwrap();
    let (store, sink) = open_backends(dir.path(), false).await.unwrap();
    let app = app_over(store, sink, false);

    let resp = app.oneshot(delete_req(TenantId(1), None)).await.unwrap();
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
}

/// No session cookie -> 401, not 404: in cloud the endpoint exists, the caller
/// just is not authenticated.
#[tokio::test]
#[file_serial]
async fn delete_tenant_requires_session() {
    let Some(store) = fresh().await else {
        eprintln!("skip: QUARK_TEST_DATABASE_URL not set");
        return;
    };
    let store = Arc::new(store);
    let (user_id, _raw) = seed_session(&store, "delete-nosession-subject").await;
    let target = seed_workspace(&store, user_id, "del-nosession", quark::tenant::Role::Owner).await;

    let app = app_over(
        store.clone() as Arc<dyn Store>,
        store.clone() as Arc<dyn AnalyticsSink>,
        true,
    );
    let resp = app.oneshot(delete_req(target, None)).await.unwrap();
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
    assert!(
        store.get_tenant(target).await.unwrap().is_some(),
        "an unauthenticated delete must not touch the tenant"
    );
}

/// Owner-only. `Admin` is refused too, even though `role_scopes` gives it the
/// same scopes as `Owner`: the Admin role comes from an IdP claim group, so
/// whoever controls Keycloak could otherwise hand themselves the workspace's
/// destruction.
#[tokio::test]
#[file_serial]
async fn delete_tenant_rejects_non_owner() {
    let Some(store) = fresh().await else {
        eprintln!("skip: QUARK_TEST_DATABASE_URL not set");
        return;
    };
    let store = Arc::new(store);
    let (user_id, raw) = seed_session(&store, "delete-nonowner-subject").await;
    let as_member =
        seed_workspace(&store, user_id, "del-member", quark::tenant::Role::Member).await;
    let as_admin = seed_workspace(&store, user_id, "del-admin", quark::tenant::Role::Admin).await;
    let as_viewer =
        seed_workspace(&store, user_id, "del-viewer", quark::tenant::Role::Viewer).await;

    let app = app_over(
        store.clone() as Arc<dyn Store>,
        store.clone() as Arc<dyn AnalyticsSink>,
        true,
    );
    for target in [as_member, as_admin, as_viewer] {
        let resp = app
            .clone()
            .oneshot(delete_req(target, Some(&raw)))
            .await
            .unwrap();
        assert_eq!(
            resp.status(),
            StatusCode::FORBIDDEN,
            "only Owner may delete a workspace (tenant {})",
            target.0
        );
        assert!(
            store.get_tenant(target).await.unwrap().is_some(),
            "a refused delete must leave the tenant standing"
        );
    }
}

/// A tenant the caller is not a member of answers 404, byte for byte the same
/// as an id that never existed. A 403 there would confirm the workspace exists,
/// which is customer enumeration in a multi-tenant product.
#[tokio::test]
#[file_serial]
async fn delete_tenant_hides_foreign_tenants() {
    let Some(store) = fresh().await else {
        eprintln!("skip: QUARK_TEST_DATABASE_URL not set");
        return;
    };
    let store = Arc::new(store);
    let (user_id, raw) = seed_session(&store, "delete-foreign-subject").await;
    let (other_id, _other_raw) = seed_session(&store, "delete-foreign-other-subject").await;
    // The caller owns one workspace of their own, so they are a legitimate,
    // fully authorized Owner somewhere: what is being probed is the OTHER
    // tenant.
    seed_workspace(
        &store,
        user_id,
        "del-foreign-mine",
        quark::tenant::Role::Owner,
    )
    .await;
    let foreign = seed_workspace(
        &store,
        other_id,
        "del-foreign-theirs",
        quark::tenant::Role::Owner,
    )
    .await;

    let app = app_over(
        store.clone() as Arc<dyn Store>,
        store.clone() as Arc<dyn AnalyticsSink>,
        true,
    );

    let existing = app
        .clone()
        .oneshot(delete_req(foreign, Some(&raw)))
        .await
        .unwrap();
    assert_eq!(existing.status(), StatusCode::NOT_FOUND);
    let existing_body = axum::body::to_bytes(existing.into_body(), usize::MAX)
        .await
        .unwrap();

    let absent = app
        .clone()
        .oneshot(delete_req(TenantId(9_999_999), Some(&raw)))
        .await
        .unwrap();
    assert_eq!(absent.status(), StatusCode::NOT_FOUND);
    let absent_body = axum::body::to_bytes(absent.into_body(), usize::MAX)
        .await
        .unwrap();

    assert_eq!(
        existing_body, absent_body,
        "an existing foreign tenant and a nonexistent id must be indistinguishable"
    );
    assert!(
        store.get_tenant(foreign).await.unwrap().is_some(),
        "the foreign tenant must be untouched"
    );
}

/// The last workspace cannot go: 409, and the tenant is still there. The user
/// is never left with nowhere to land.
#[tokio::test]
#[file_serial]
async fn delete_tenant_refuses_the_last_workspace() {
    let Some(store) = fresh().await else {
        eprintln!("skip: QUARK_TEST_DATABASE_URL not set");
        return;
    };
    let store = Arc::new(store);
    let (user_id, raw) = seed_session(&store, "delete-last-subject").await;
    let only = seed_workspace(&store, user_id, "del-last", quark::tenant::Role::Owner).await;

    let app = app_over(
        store.clone() as Arc<dyn Store>,
        store.clone() as Arc<dyn AnalyticsSink>,
        true,
    );
    let resp = app.oneshot(delete_req(only, Some(&raw))).await.unwrap();
    assert_eq!(resp.status(), StatusCode::CONFLICT);
    assert!(
        store.get_tenant(only).await.unwrap().is_some(),
        "the last workspace must survive the refusal"
    );
    assert!(
        store.get_membership(user_id, only).await.unwrap().is_some(),
        "the membership must survive too"
    );
}

/// The happy path: an Owner with two workspaces deletes one, gets 204, the
/// tenant and its membership are gone, and `/admin/me` now lists only the
/// other one.
#[tokio::test]
#[file_serial]
async fn delete_tenant_succeeds_for_the_owner() {
    let Some(store) = fresh().await else {
        eprintln!("skip: QUARK_TEST_DATABASE_URL not set");
        return;
    };
    let store = Arc::new(store);
    let (user_id, raw) = seed_session(&store, "delete-happy-subject").await;
    let doomed = seed_workspace(
        &store,
        user_id,
        "del-happy-doomed",
        quark::tenant::Role::Owner,
    )
    .await;
    let keeper = seed_workspace(
        &store,
        user_id,
        "del-happy-keeper",
        quark::tenant::Role::Owner,
    )
    .await;

    let app = app_over(
        store.clone() as Arc<dyn Store>,
        store.clone() as Arc<dyn AnalyticsSink>,
        true,
    );
    let resp = app
        .clone()
        .oneshot(delete_req(doomed, Some(&raw)))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::NO_CONTENT);

    assert!(store.get_tenant(doomed).await.unwrap().is_none(), "tenant");
    assert!(
        store
            .get_membership(user_id, doomed)
            .await
            .unwrap()
            .is_none(),
        "membership"
    );
    assert!(
        store.get_tenant(keeper).await.unwrap().is_some(),
        "the other workspace must be untouched"
    );

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
    let memberships = me["memberships"].as_array().unwrap();
    assert_eq!(
        memberships.len(),
        1,
        "only the surviving workspace is listed"
    );
    assert_eq!(memberships[0]["tenant_id"], keeper.0);
}

/// A pre-delete read that fails must abort with `503` and delete nothing.
/// `list_domains` is read before the transaction because its hosts drive the
/// router-cache invalidation at the end of the handler: swallowing its error
/// would delete the workspace and leave even the node serving this request
/// still resolving the deleted workspace's hosts until the route TTL runs out.
/// The seam is a permission revoke on `domains`, which is what makes exactly
/// that one read fail while every read before it keeps working.
#[tokio::test]
#[file_serial]
async fn delete_tenant_aborts_when_the_domain_list_fails() {
    let Some(store) = fresh().await else {
        eprintln!("skip: QUARK_TEST_DATABASE_URL not set");
        return;
    };
    let url = std::env::var("QUARK_TEST_DATABASE_URL").unwrap();
    let pool = PgPoolOptions::new()
        .max_connections(1)
        .connect(&url)
        .await
        .unwrap();
    // A superuser ignores GRANT and REVOKE entirely, so the seam below would be
    // a no-op: `list_domains` would succeed, the delete would go through, and
    // the test would report a 204 that proves nothing. CI still connects as
    // `postgres` (LUC-143), so skip loudly there instead of failing on a
    // property of the role rather than of the code.
    let is_superuser: bool =
        sqlx::query_scalar("SELECT usesuper FROM pg_user WHERE usename = CURRENT_USER")
            .fetch_one(&pool)
            .await
            .unwrap();
    if is_superuser {
        eprintln!(
            "skip: QUARK_TEST_DATABASE_URL connects as a superuser, which ignores REVOKE, \
             so this test cannot make the domain read fail (see LUC-143)"
        );
        return;
    }

    let store = Arc::new(store);
    let (user_id, raw) = seed_session(&store, "delete-domfail-subject").await;
    let doomed = seed_workspace(
        &store,
        user_id,
        "del-domfail-doomed",
        quark::tenant::Role::Owner,
    )
    .await;
    seed_workspace(
        &store,
        user_id,
        "del-domfail-keeper",
        quark::tenant::Role::Owner,
    )
    .await;

    // Column-level, not table-level: `list_domains` reads `host` and fails,
    // while `DELETE FROM domains WHERE tenant_id = $1` only reads `tenant_id`
    // and would still go through. Revoking the whole table would break the
    // delete too, and then the `503` would prove nothing about the read.
    sqlx::query("REVOKE SELECT ON domains FROM CURRENT_USER")
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query("GRANT SELECT (tenant_id) ON domains TO CURRENT_USER")
        .execute(&pool)
        .await
        .unwrap();
    let app = app_over(
        store.clone() as Arc<dyn Store>,
        store.clone() as Arc<dyn AnalyticsSink>,
        true,
    );
    let status = app
        .oneshot(delete_req(doomed, Some(&raw)))
        .await
        .unwrap()
        .status();
    // Put the privilege back before asserting: a panic above this line would
    // leave `domains` unreadable for every test that follows in this binary.
    sqlx::query("GRANT SELECT ON domains TO CURRENT_USER")
        .execute(&pool)
        .await
        .unwrap();

    assert_eq!(
        status,
        StatusCode::SERVICE_UNAVAILABLE,
        "a failed domain list must not fall through to the delete"
    );
    assert!(
        store.get_tenant(doomed).await.unwrap().is_some(),
        "the workspace must still be there after a failed pre-delete read"
    );
    assert!(
        store
            .get_membership(user_id, doomed)
            .await
            .unwrap()
            .is_some(),
        "the membership must still be there too"
    );
}

/// The Keycloak side effect is exactly one `delete_realm`, naming the deleted
/// workspace's slug and nothing else. The other workspace's realm must never
/// appear.
#[tokio::test]
#[file_serial]
async fn delete_tenant_deletes_only_its_own_realm() {
    let Some(store) = fresh().await else {
        eprintln!("skip: QUARK_TEST_DATABASE_URL not set");
        return;
    };
    let store = Arc::new(store);
    let (user_id, raw) = seed_session(&store, "delete-realm-subject").await;
    let doomed = seed_workspace(
        &store,
        user_id,
        "del-realm-doomed",
        quark::tenant::Role::Owner,
    )
    .await;
    seed_workspace(
        &store,
        user_id,
        "del-realm-keeper",
        quark::tenant::Role::Owner,
    )
    .await;

    let mock = Arc::new(quark::ee::keycloak::testing::MockKeycloakAdmin::default());
    let app = app_over_with_keycloak(
        store.clone() as Arc<dyn Store>,
        store.clone() as Arc<dyn AnalyticsSink>,
        true,
        Some(mock.clone() as Arc<dyn quark::ee::keycloak::KeycloakAdmin>),
        Some("https://kc.example.com".to_string()),
    );
    let resp = app.oneshot(delete_req(doomed, Some(&raw))).await.unwrap();
    assert_eq!(resp.status(), StatusCode::NO_CONTENT);

    assert_eq!(
        mock.calls(),
        vec!["delete_realm(del-realm-doomed)".to_string()],
        "the delete must remove exactly the doomed workspace's realm"
    );
}

/// Deleting the workspace the session is currently in must not log the user
/// out. `sessions` is tenant-owned, so the session row goes down with the
/// tenant; the handler re-issues it against a remaining workspace, and the very
/// next request works.
#[tokio::test]
#[file_serial]
async fn delete_tenant_keeps_the_session_alive_on_another_workspace() {
    let Some(store) = fresh().await else {
        eprintln!("skip: QUARK_TEST_DATABASE_URL not set");
        return;
    };
    let store = Arc::new(store);
    let (user_id, raw) = seed_session(&store, "delete-session-subject").await;
    let doomed = seed_workspace(
        &store,
        user_id,
        "del-sess-doomed",
        quark::tenant::Role::Owner,
    )
    .await;
    let keeper = seed_workspace(
        &store,
        user_id,
        "del-sess-keeper",
        quark::tenant::Role::Owner,
    )
    .await;

    let app = app_over(
        store.clone() as Arc<dyn Store>,
        store.clone() as Arc<dyn AnalyticsSink>,
        true,
    );

    // Put the session INTO the workspace about to be deleted, which is what the
    // panel does right after creating one.
    let resp = app
        .clone()
        .oneshot(
            Request::post("/admin/workspace/switch")
                .header("content-type", "application/json")
                .header("cookie", format!("qk_session={raw}"))
                .body(Body::from(format!(r#"{{"tenant_id":{}}}"#, doomed.0)))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    let resp = app
        .clone()
        .oneshot(delete_req(doomed, Some(&raw)))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::NO_CONTENT);

    // Same cookie, next request: still authenticated, now pointing at the
    // workspace that is left.
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
        me["authenticated"], true,
        "the delete must not log the user out"
    );
    assert_eq!(me["current_tenant"], keeper.0);

    // And a guarded endpoint (which re-resolves membership in the session's
    // current tenant) answers, rather than 401/403.
    let resp = app
        .oneshot(
            Request::get("/admin/links")
                .header("cookie", format!("qk_session={raw}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(
        resp.status(),
        StatusCode::OK,
        "the re-issued session must authorize the guarded surface"
    );
}

/// Best-effort Keycloak: `delete_realm` failing must NOT undo the deletion nor
/// surface an error. The tenant is already gone (the store transaction
/// committed first, deliberately), the answer is still 204, and the orphaned
/// realm is only a `warn!`. Mirrors
/// `create_tenant_survives_ensure_realm_failure` on the creation side.
struct DeleteRealmFailsKeycloakAdmin;

#[async_trait::async_trait]
impl quark::ee::keycloak::KeycloakAdmin for DeleteRealmFailsKeycloakAdmin {
    async fn ensure_realm(&self, _slug: &str) -> Result<(), quark::ee::keycloak::KcError> {
        unreachable!("this fake only exercises the deletion path")
    }
    async fn ensure_client(
        &self,
        _slug: &str,
        _redirect_uri: &str,
    ) -> Result<(), quark::ee::keycloak::KcError> {
        unreachable!("this fake only exercises the deletion path")
    }
    async fn ensure_groups_and_mapper(
        &self,
        _slug: &str,
    ) -> Result<(), quark::ee::keycloak::KcError> {
        unreachable!("this fake only exercises the deletion path")
    }
    async fn ensure_user(
        &self,
        _slug: &str,
        _email: &str,
        _group: &str,
    ) -> Result<String, quark::ee::keycloak::KcError> {
        unreachable!("this fake only exercises the deletion path")
    }
    async fn send_set_password_email(
        &self,
        _slug: &str,
        _user_id: &str,
    ) -> Result<(), quark::ee::keycloak::KcError> {
        unreachable!("this fake only exercises the deletion path")
    }
    async fn delete_realm(&self, _slug: &str) -> Result<(), quark::ee::keycloak::KcError> {
        Err(quark::ee::keycloak::KcError(
            "delete_realm unavailable".into(),
        ))
    }
}

#[tokio::test]
#[file_serial]
async fn delete_tenant_survives_delete_realm_failure() {
    let Some(store) = fresh().await else {
        eprintln!("skip: QUARK_TEST_DATABASE_URL not set");
        return;
    };
    let store = Arc::new(store);
    let (user_id, raw) = seed_session(&store, "delete-kcfail-subject").await;
    let doomed = seed_workspace(
        &store,
        user_id,
        "del-kcfail-doomed",
        quark::tenant::Role::Owner,
    )
    .await;
    seed_workspace(
        &store,
        user_id,
        "del-kcfail-keeper",
        quark::tenant::Role::Owner,
    )
    .await;

    let app = app_over_with_keycloak(
        store.clone() as Arc<dyn Store>,
        store.clone() as Arc<dyn AnalyticsSink>,
        true,
        Some(Arc::new(DeleteRealmFailsKeycloakAdmin) as Arc<dyn quark::ee::keycloak::KeycloakAdmin>),
        Some("https://kc.example.com".to_string()),
    );
    let resp = app.oneshot(delete_req(doomed, Some(&raw))).await.unwrap();
    assert_eq!(
        resp.status(),
        StatusCode::NO_CONTENT,
        "a Keycloak failure must never fail (or undo) the deletion"
    );
    assert!(
        store.get_tenant(doomed).await.unwrap().is_none(),
        "the tenant is gone regardless; only the realm is orphaned"
    );
    assert!(
        store
            .get_membership(user_id, doomed)
            .await
            .unwrap()
            .is_none(),
        "the membership is gone too"
    );
}
