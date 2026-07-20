//! Store-level tests for SSO email-domain discovery (LUC-57, Task 1) plus
//! the admin HTTP endpoints (LUC-57, Task 2). Postgres-gated on
//! `QUARK_TEST_DATABASE_URL`; skips when unset.
use axum::body::Body;
use axum::http::{Request, StatusCode};
use quark::analytics::AnalyticsSink;
use quark::api::router;
use quark::auth::{hash_token, ApiToken, Scope};
use quark::cache::Cache;
use quark::dns::{Dns, DnsError, NullDns};
use quark::domain::DomainStatus;
use quark::oidc::TenantOidcConfig;
use quark::sso::SsoEmailDomain;
use quark::store::postgres::PostgresStore;
use quark::store::Store;
use quark::tenant::{Tenant, TenantId};
use quark::webhooks::delivery::WebhookDispatcher;
use serial_test::file_serial;
use std::collections::HashMap;
use std::sync::Arc;
use tower::ServiceExt;

mod common;

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
#[file_serial]
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
#[file_serial]
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
#[file_serial]
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
#[file_serial]
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

// --- LUC-57 Task 2: admin CRUD + DNS-TXT verify endpoints -------------------

const KEY: u64 = 0x1234;

/// A `Dns` fake whose TXT records are fixed at construction (mirrors
/// `domains_it::FakeDns`).
struct FakeDns {
    records: HashMap<String, Vec<String>>,
}

impl FakeDns {
    fn with_record(name: &str, values: Vec<String>) -> Self {
        let mut records = HashMap::new();
        records.insert(name.to_string(), values);
        FakeDns { records }
    }
}

#[async_trait::async_trait]
impl Dns for FakeDns {
    async fn lookup_txt(&self, name: &str) -> Result<Vec<String>, DnsError> {
        Ok(self.records.get(name).cloned().unwrap_or_default())
    }
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

/// Gives `tenant` an OIDC config, the precondition `POST /admin/sso-domains`
/// gates on.
async fn seed_oidc_config(store: &PostgresStore, tenant: TenantId) {
    store
        .put_oidc_config(&TenantOidcConfig {
            tenant_id: tenant,
            issuer: "https://idp.example.com".to_string(),
            client_id: "client".to_string(),
            client_secret: "secret".to_string(),
            scopes: vec!["openid".to_string()],
            admin_claim: "role".to_string(),
            admin_value: "admin".to_string(),
            readonly_value: "member".to_string(),
            required_value: None,
            post_login_url: None,
            post_logout_url: None,
        })
        .await
        .unwrap();
}

/// Builds a router with a scoped API token already seeded for `tenant`, so
/// tests can hit `/admin/sso-domains` as that tenant via `x-admin-token`
/// (mirrors `domains_it::admin_app_for_tenant`).
async fn admin_app_for_tenant(
    store: Arc<PostgresStore>,
    dns: Arc<dyn Dns>,
    multi_tenant: bool,
    tenant: TenantId,
    token_id: u64,
    scopes: Vec<Scope>,
) -> (axum::Router, String) {
    let raw = format!("qtok_sso_test_{}", token_id);
    store
        .put_api_token(
            tenant,
            &ApiToken {
                id: token_id,
                name: "sso-domains-test-token".to_string(),
                token_hash: hash_token(&raw),
                scopes,
                rate_limit_per_min: None,
                created: 0,
                tenant_id: tenant,
            },
        )
        .await
        .unwrap();

    let store_dyn: Arc<dyn Store> = store.clone();
    let sink_dyn: Arc<dyn AnalyticsSink> = store;
    let cache = Cache::new(store_dyn.clone(), 1000, None);
    let host_router = Arc::new(quark::domain_router::HostRouter::new(
        store_dyn.clone(),
        Some("quark.example.com".to_string()),
        None,
    ));
    let (analytics_tx, _rx) = tokio::sync::mpsc::channel(100);
    let state = common::TestState::new(store_dyn, sink_dyn)
        .cache(cache)
        .host_router(host_router)
        .analytics_tx(analytics_tx)
        .webhooks(test_webhook_dispatcher())
        .key(KEY)
        .public_host(Some("quark.example.com".to_string()))
        .multi_tenant(multi_tenant)
        .dns(dns)
        .build();
    (router(state), raw)
}

async fn create_sso_domain(
    app: &axum::Router,
    token: &str,
    domain: &str,
) -> (StatusCode, serde_json::Value) {
    let resp = app
        .clone()
        .oneshot(
            Request::post("/admin/sso-domains")
                .header("content-type", "application/json")
                .header("x-admin-token", token)
                .body(Body::from(format!(r#"{{"domain":"{domain}"}}"#)))
                .unwrap(),
        )
        .await
        .unwrap();
    let status = resp.status();
    let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .unwrap();
    let json = if body.is_empty() {
        serde_json::Value::Null
    } else {
        serde_json::from_slice(&body).unwrap_or(serde_json::Value::Null)
    };
    (status, json)
}

/// Creating an SSO domain for a tenant that has an `oidc_config` succeeds and
/// returns the pending row plus the TXT verification instructions.
#[tokio::test]
#[file_serial]
async fn create_returns_pending_with_instructions_when_sso_configured() {
    let Some(store) = fresh().await else {
        eprintln!("skip: QUARK_TEST_DATABASE_URL not set");
        return;
    };
    let store = Arc::new(store);
    let tenant = make_tenant(&store, "http-sso-a").await;
    seed_oidc_config(&store, tenant).await;
    let (app, token) = admin_app_for_tenant(
        store.clone(),
        Arc::new(NullDns),
        true,
        tenant,
        9101,
        vec![Scope::Full],
    )
    .await;

    let (status, body) = create_sso_domain(&app, &token, "acme.com").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["domain"], "acme.com");
    assert_eq!(body["status"], "pending");
    assert_eq!(body["txt_name"], "_quark-sso.acme.com");
    assert!(
        body["txt_value"].as_str().is_some_and(|v| !v.is_empty()),
        "must return a non-empty verification token"
    );

    let id = body["id"].as_u64().unwrap();
    let stored = store.get_sso_domain(tenant, id).await.unwrap().unwrap();
    assert_eq!(stored.status, DomainStatus::Pending);
}

/// Without an `oidc_config`, creating an SSO domain is rejected before any
/// row is written.
#[tokio::test]
#[file_serial]
async fn create_without_oidc_config_is_rejected() {
    let Some(store) = fresh().await else {
        eprintln!("skip: QUARK_TEST_DATABASE_URL not set");
        return;
    };
    let store = Arc::new(store);
    let tenant = make_tenant(&store, "http-sso-noconf").await;
    let (app, token) = admin_app_for_tenant(
        store.clone(),
        Arc::new(NullDns),
        true,
        tenant,
        9102,
        vec![Scope::Full],
    )
    .await;

    let (status, _) = create_sso_domain(&app, &token, "noconf.com").await;
    assert_eq!(status, StatusCode::CONFLICT);
    assert_eq!(
        store.list_sso_domains(tenant).await.unwrap().len(),
        0,
        "no row must be created when SSO isn't configured"
    );
}

/// A domain already claimed by another tenant is a 409, even when the
/// second tenant also has an `oidc_config`.
#[tokio::test]
#[file_serial]
async fn create_duplicate_domain_across_tenants_is_409() {
    let Some(store) = fresh().await else {
        eprintln!("skip: QUARK_TEST_DATABASE_URL not set");
        return;
    };
    let store = Arc::new(store);
    let owner = make_tenant(&store, "http-sso-owner").await;
    let squatter = make_tenant(&store, "http-sso-squatter").await;
    seed_oidc_config(&store, owner).await;
    seed_oidc_config(&store, squatter).await;

    let (app_owner, token_owner) = admin_app_for_tenant(
        store.clone(),
        Arc::new(NullDns),
        true,
        owner,
        9103,
        vec![Scope::Full],
    )
    .await;
    let (status1, _) = create_sso_domain(&app_owner, &token_owner, "dup.com").await;
    assert_eq!(status1, StatusCode::OK);

    let (app_squatter, token_squatter) = admin_app_for_tenant(
        store.clone(),
        Arc::new(NullDns),
        true,
        squatter,
        9104,
        vec![Scope::Full],
    )
    .await;
    let (status2, _) = create_sso_domain(&app_squatter, &token_squatter, "dup.com").await;
    assert_eq!(status2, StatusCode::CONFLICT);
}

/// `verify` with a `FakeDns` returning the domain's token marks it `Verified`.
#[tokio::test]
#[file_serial]
async fn verify_with_matching_txt_marks_verified() {
    let Some(store) = fresh().await else {
        eprintln!("skip: QUARK_TEST_DATABASE_URL not set");
        return;
    };
    let store = Arc::new(store);
    let tenant = make_tenant(&store, "http-sso-verify-ok").await;
    seed_oidc_config(&store, tenant).await;

    let (app, token) = admin_app_for_tenant(
        store.clone(),
        Arc::new(NullDns),
        true,
        tenant,
        9105,
        vec![Scope::Full],
    )
    .await;
    let (status, body) = create_sso_domain(&app, &token, "verify-ok.com").await;
    assert_eq!(status, StatusCode::OK);
    let domain_id = body["id"].as_u64().unwrap();
    let verify_token = body["txt_value"].as_str().unwrap().to_string();

    let fake_dns = Arc::new(FakeDns::with_record(
        "_quark-sso.verify-ok.com",
        vec![verify_token],
    ));
    let (app2, token2) = admin_app_for_tenant(
        store.clone(),
        fake_dns,
        true,
        tenant,
        9106,
        vec![Scope::Full],
    )
    .await;
    let resp = app2
        .oneshot(
            Request::post(format!("/admin/sso-domains/{domain_id}/verify"))
                .header("x-admin-token", token2)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .unwrap();
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(json["status"], "verified");

    let stored = store
        .get_sso_domain(tenant, domain_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(stored.status, DomainStatus::Verified);
    assert!(stored.verified_at.is_some());
}

/// `verify` with no matching TXT record leaves the domain `pending`.
#[tokio::test]
#[file_serial]
async fn verify_with_wrong_txt_stays_pending() {
    let Some(store) = fresh().await else {
        eprintln!("skip: QUARK_TEST_DATABASE_URL not set");
        return;
    };
    let store = Arc::new(store);
    let tenant = make_tenant(&store, "http-sso-verify-bad").await;
    seed_oidc_config(&store, tenant).await;

    let (app, token) = admin_app_for_tenant(
        store.clone(),
        Arc::new(NullDns),
        true,
        tenant,
        9107,
        vec![Scope::Full],
    )
    .await;
    let (status, body) = create_sso_domain(&app, &token, "verify-bad.com").await;
    assert_eq!(status, StatusCode::OK);
    let domain_id = body["id"].as_u64().unwrap();

    let fake_dns = Arc::new(FakeDns::with_record(
        "_quark-sso.verify-bad.com",
        vec!["not-the-token".to_string()],
    ));
    let (app2, token2) = admin_app_for_tenant(
        store.clone(),
        fake_dns,
        true,
        tenant,
        9108,
        vec![Scope::Full],
    )
    .await;
    let resp = app2
        .oneshot(
            Request::post(format!("/admin/sso-domains/{domain_id}/verify"))
                .header("x-admin-token", token2)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .unwrap();
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(json["status"], "pending");

    let stored = store
        .get_sso_domain(tenant, domain_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(stored.status, DomainStatus::Pending);
}

/// `list`/`delete` are tenant-scoped: tenant B's admin view never sees
/// tenant A's SSO domain, and cannot delete it by id either.
#[tokio::test]
#[file_serial]
async fn list_and_delete_are_tenant_scoped() {
    let Some(store) = fresh().await else {
        eprintln!("skip: QUARK_TEST_DATABASE_URL not set");
        return;
    };
    let store = Arc::new(store);
    let tenant_a = make_tenant(&store, "http-sso-scope-a").await;
    let tenant_b = make_tenant(&store, "http-sso-scope-b").await;
    seed_oidc_config(&store, tenant_a).await;

    let (app_a, token_a) = admin_app_for_tenant(
        store.clone(),
        Arc::new(NullDns),
        true,
        tenant_a,
        9109,
        vec![Scope::Full],
    )
    .await;
    let (status, body) = create_sso_domain(&app_a, &token_a, "scope.com").await;
    assert_eq!(status, StatusCode::OK);
    let domain_id = body["id"].as_u64().unwrap();

    let (app_b, token_b) = admin_app_for_tenant(
        store.clone(),
        Arc::new(NullDns),
        true,
        tenant_b,
        9110,
        vec![Scope::Full],
    )
    .await;

    // Tenant B's list must not include tenant A's domain.
    let resp = app_b
        .clone()
        .oneshot(
            Request::get("/admin/sso-domains")
                .header("x-admin-token", &token_b)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .unwrap();
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert!(
        json.as_array().unwrap().is_empty(),
        "tenant B must not see tenant A's SSO domain in its own list"
    );

    // Tenant A's own list does include it.
    let resp = app_a
        .oneshot(
            Request::get("/admin/sso-domains")
                .header("x-admin-token", &token_a)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .unwrap();
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(json.as_array().unwrap().len(), 1);

    // Tenant B cannot delete tenant A's domain by id.
    let resp = app_b
        .oneshot(
            Request::delete(format!("/admin/sso-domains/{domain_id}"))
                .header("x-admin-token", &token_b)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
    assert!(store
        .get_sso_domain(tenant_a, domain_id)
        .await
        .unwrap()
        .is_some());

    // Tenant A can delete its own domain.
    let (app_a2, token_a2) = admin_app_for_tenant(
        store.clone(),
        Arc::new(NullDns),
        true,
        tenant_a,
        9111,
        vec![Scope::Full],
    )
    .await;
    let resp = app_a2
        .oneshot(
            Request::delete(format!("/admin/sso-domains/{domain_id}"))
                .header("x-admin-token", &token_a2)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::NO_CONTENT);
    assert!(store
        .get_sso_domain(tenant_a, domain_id)
        .await
        .unwrap()
        .is_none());
}

/// All four `/admin/sso-domains` endpoints 404 in OSS (`multi_tenant = false`).
#[tokio::test]
#[file_serial]
async fn endpoints_404_in_oss() {
    let Some(store) = fresh().await else {
        eprintln!("skip: QUARK_TEST_DATABASE_URL not set");
        return;
    };
    let store = Arc::new(store);
    let tenant = make_tenant(&store, "http-sso-oss").await;
    let (app, token) = admin_app_for_tenant(
        store,
        Arc::new(NullDns),
        false,
        tenant,
        9112,
        vec![Scope::Full],
    )
    .await;

    let resp = app
        .clone()
        .oneshot(
            Request::get("/admin/sso-domains")
                .header("x-admin-token", &token)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);

    let resp = app
        .clone()
        .oneshot(
            Request::post("/admin/sso-domains")
                .header("content-type", "application/json")
                .header("x-admin-token", &token)
                .body(Body::from(r#"{"domain":"oss.com"}"#))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);

    let resp = app
        .clone()
        .oneshot(
            Request::post("/admin/sso-domains/1/verify")
                .header("x-admin-token", &token)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);

    let resp = app
        .oneshot(
            Request::delete("/admin/sso-domains/1")
                .header("x-admin-token", &token)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
}

/// An API token with insufficient scope (not `Full`) is rejected with 403,
/// mirroring the P3 custom-domains scope contract.
#[tokio::test]
#[file_serial]
async fn insufficient_scope_is_403() {
    let Some(store) = fresh().await else {
        eprintln!("skip: QUARK_TEST_DATABASE_URL not set");
        return;
    };
    let store = Arc::new(store);
    let tenant = make_tenant(&store, "http-sso-lowscope").await;
    seed_oidc_config(&store, tenant).await;
    let (app, token) = admin_app_for_tenant(
        store,
        Arc::new(NullDns),
        true,
        tenant,
        9113,
        vec![Scope::LinksRead],
    )
    .await;

    let resp = app
        .clone()
        .oneshot(
            Request::get("/admin/sso-domains")
                .header("x-admin-token", &token)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::FORBIDDEN);

    let resp = app
        .oneshot(
            Request::post("/admin/sso-domains")
                .header("content-type", "application/json")
                .header("x-admin-token", &token)
                .body(Body::from(r#"{"domain":"lowscope.com"}"#))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::FORBIDDEN);
}

// --- LUC-57 Task 3: public discovery endpoint -------------------------------

/// Builds a router for the PUBLIC `/admin/sso/discover` endpoint: no API
/// token is seeded (the endpoint takes none), but the caller picks the
/// rate limiter so both the permissive default and a tripped-burst case can
/// be exercised.
fn discover_app(
    store: Arc<PostgresStore>,
    multi_tenant: bool,
    ratelimiter: quark::abuse::ratelimit::RateLimiter,
) -> axum::Router {
    let store_dyn: Arc<dyn Store> = store.clone();
    let sink_dyn: Arc<dyn AnalyticsSink> = store;
    let cache = Cache::new(store_dyn.clone(), 1000, None);
    let host_router = Arc::new(quark::domain_router::HostRouter::new(
        store_dyn.clone(),
        Some("quark.example.com".to_string()),
        None,
    ));
    let (analytics_tx, _rx) = tokio::sync::mpsc::channel(100);
    let state = common::TestState::new(store_dyn, sink_dyn)
        .cache(cache)
        .host_router(host_router)
        .analytics_tx(analytics_tx)
        .webhooks(test_webhook_dispatcher())
        .key(KEY)
        .ratelimiter(ratelimiter)
        .public_host(Some("quark.example.com".to_string()))
        .multi_tenant(multi_tenant)
        .build();
    router(state)
}

async fn discover(app: &axum::Router, email: &str) -> (StatusCode, serde_json::Value, String) {
    let resp = app
        .clone()
        .oneshot(
            Request::get(format!(
                "/admin/sso/discover?email={}",
                urlencoding_light(email)
            ))
            .header("cf-connecting-ip", "5.5.5.5")
            .body(Body::empty())
            .unwrap(),
        )
        .await
        .unwrap();
    let status = resp.status();
    let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .unwrap();
    let raw = String::from_utf8_lossy(&body).to_string();
    let json = if body.is_empty() {
        serde_json::Value::Null
    } else {
        serde_json::from_slice(&body).unwrap_or(serde_json::Value::Null)
    };
    (status, json, raw)
}

/// Minimal query-param encoding sufficient for the test emails used here
/// (only `@` needs escaping to survive as a single query value).
fn urlencoding_light(s: &str) -> String {
    s.replace('@', "%40")
}

/// A verified domain whose tenant has an `oidc_config` resolves to that
/// tenant's slug, and the response body carries `org` only -- no
/// `tenant_id` anywhere.
#[tokio::test]
#[file_serial]
async fn discover_verified_with_oidc_returns_org_slug() {
    let Some(store) = fresh().await else {
        eprintln!("skip: QUARK_TEST_DATABASE_URL not set");
        return;
    };
    let store = Arc::new(store);
    let tenant = make_tenant(&store, "discover-a").await;
    seed_oidc_config(&store, tenant).await;
    let id = put(&store, tenant, "acme.com").await;
    store
        .set_sso_domain_status(tenant, id, DomainStatus::Verified, Some(1))
        .await
        .unwrap();

    let app = discover_app(
        store,
        true,
        quark::abuse::ratelimit::RateLimiter::disabled(),
    );
    let (status, body, raw) = discover(&app, "user@acme.com").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["org"], "discover-a");
    assert!(!raw.contains("tenant_id"), "must never leak tenant_id");
}

/// A `Pending` (unverified) domain must never route -- anti-hijack
/// guarantee -- so discovery returns the uniform empty body.
#[tokio::test]
#[file_serial]
async fn discover_pending_domain_returns_empty() {
    let Some(store) = fresh().await else {
        eprintln!("skip: QUARK_TEST_DATABASE_URL not set");
        return;
    };
    let store = Arc::new(store);
    let tenant = make_tenant(&store, "discover-pending").await;
    seed_oidc_config(&store, tenant).await;
    put(&store, tenant, "pending.com").await; // left Pending

    let app = discover_app(
        store,
        true,
        quark::abuse::ratelimit::RateLimiter::disabled(),
    );
    let (status, body, raw) = discover(&app, "user@pending.com").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body, serde_json::json!({}));
    assert!(!raw.contains("tenant_id"));
}

/// An unknown domain returns the same uniform empty body as every other
/// non-match -- an unauthenticated caller cannot distinguish "no such
/// domain" from "domain exists but isn't ready".
#[tokio::test]
#[file_serial]
async fn discover_unknown_domain_returns_empty() {
    let Some(store) = fresh().await else {
        eprintln!("skip: QUARK_TEST_DATABASE_URL not set");
        return;
    };
    let store = Arc::new(store);
    let app = discover_app(
        store,
        true,
        quark::abuse::ratelimit::RateLimiter::disabled(),
    );
    let (status, body, raw) = discover(&app, "user@nowhere.example").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body, serde_json::json!({}));
    assert!(!raw.contains("tenant_id"));
}

/// A verified domain whose tenant subsequently lost its `oidc_config` (SSO
/// disabled) must not route either -- there'd be nowhere for the login to
/// go, and this would otherwise reveal that the domain is claimed.
#[tokio::test]
#[file_serial]
async fn discover_verified_without_oidc_config_returns_empty() {
    let Some(store) = fresh().await else {
        eprintln!("skip: QUARK_TEST_DATABASE_URL not set");
        return;
    };
    let store = Arc::new(store);
    let tenant = make_tenant(&store, "discover-noconf").await;
    seed_oidc_config(&store, tenant).await;
    let id = put(&store, tenant, "noconf.com").await;
    store
        .set_sso_domain_status(tenant, id, DomainStatus::Verified, Some(1))
        .await
        .unwrap();
    // The tenant later drops its OIDC config (SSO disabled).
    store.delete_oidc_config(tenant).await.unwrap();

    let app = discover_app(
        store,
        true,
        quark::abuse::ratelimit::RateLimiter::disabled(),
    );
    let (status, body, raw) = discover(&app, "user@noconf.com").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body, serde_json::json!({}));
    assert!(!raw.contains("tenant_id"));
}

/// A malformed email (no `@`, or otherwise not `normalize_email_domain`-able)
/// returns the same uniform empty body -- never a 400.
#[tokio::test]
#[file_serial]
async fn discover_malformed_email_returns_empty() {
    let Some(store) = fresh().await else {
        eprintln!("skip: QUARK_TEST_DATABASE_URL not set");
        return;
    };
    let store = Arc::new(store);
    let app = discover_app(
        store,
        true,
        quark::abuse::ratelimit::RateLimiter::disabled(),
    );
    let (status, body, raw) = discover(&app, "nope").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body, serde_json::json!({}));
    assert!(!raw.contains("tenant_id"));
}

/// No `email` query param at all returns the same uniform empty body as a
/// malformed email -- the missing-param branch must not 400 or diverge.
#[tokio::test]
#[file_serial]
async fn discover_without_email_param_returns_empty() {
    let Some(store) = fresh().await else {
        eprintln!("skip: QUARK_TEST_DATABASE_URL not set");
        return;
    };
    let store = Arc::new(store);
    let app = discover_app(
        store,
        true,
        quark::abuse::ratelimit::RateLimiter::disabled(),
    );
    let resp = app
        .clone()
        .oneshot(
            Request::get("/admin/sso/discover")
                .header("cf-connecting-ip", "5.5.5.5")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .unwrap();
    let raw = String::from_utf8_lossy(&body).to_string();
    assert_eq!(
        serde_json::from_slice::<serde_json::Value>(&body).unwrap(),
        serde_json::json!({})
    );
    assert!(!raw.contains("tenant_id"));
}

/// OSS (`multi_tenant = false`) 404s the discovery endpoint entirely.
#[tokio::test]
#[file_serial]
async fn discover_404_in_oss() {
    let Some(store) = fresh().await else {
        eprintln!("skip: QUARK_TEST_DATABASE_URL not set");
        return;
    };
    let store = Arc::new(store);
    let app = discover_app(
        store,
        false,
        quark::abuse::ratelimit::RateLimiter::disabled(),
    );
    let (status, _, _) = discover(&app, "user@acme.com").await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

/// Per-IP rate limiting is consulted on the discovery path: with a
/// one-request burst, the first call succeeds and the second (same IP)
/// trips `429` (mirrors `api_it::rate_limit_429_after_exceeding`).
#[tokio::test]
#[file_serial]
async fn discover_rate_limit_429_after_exceeding() {
    let Some(store) = fresh().await else {
        eprintln!("skip: QUARK_TEST_DATABASE_URL not set");
        return;
    };
    let store = Arc::new(store);
    let app = discover_app(store, true, quark::abuse::ratelimit::RateLimiter::memory(1));
    let (status1, _, _) = discover(&app, "user@acme.com").await;
    assert_eq!(status1, StatusCode::OK);
    let (status2, _, _) = discover(&app, "user@acme.com").await;
    assert_eq!(status2, StatusCode::TOO_MANY_REQUESTS);
}

/// Fold-in (Task 2 review, Minor): `POST /admin/sso-domains` is now
/// rate-limited like its P3 mirror `admin_domains_create`. With a
/// one-request burst the first create succeeds and a second immediately
/// after (same IP) trips `429` before ever reaching the store.
#[tokio::test]
#[file_serial]
async fn create_is_rate_limited() {
    let Some(store) = fresh().await else {
        eprintln!("skip: QUARK_TEST_DATABASE_URL not set");
        return;
    };
    let store = Arc::new(store);
    let tenant = make_tenant(&store, "http-sso-ratelimited").await;
    seed_oidc_config(&store, tenant).await;

    let raw = "qtok_sso_ratelimit_test".to_string();
    store
        .put_api_token(
            tenant,
            &ApiToken {
                id: 9114,
                name: "sso-domains-ratelimit-token".to_string(),
                token_hash: hash_token(&raw),
                scopes: vec![Scope::Full],
                rate_limit_per_min: None,
                created: 0,
                tenant_id: tenant,
            },
        )
        .await
        .unwrap();

    let store_dyn: Arc<dyn Store> = store.clone();
    let sink_dyn: Arc<dyn AnalyticsSink> = store.clone();
    let cache = Cache::new(store_dyn.clone(), 1000, None);
    let host_router = Arc::new(quark::domain_router::HostRouter::new(
        store_dyn.clone(),
        Some("quark.example.com".to_string()),
        None,
    ));
    let (analytics_tx, _rx) = tokio::sync::mpsc::channel(100);
    let state = common::TestState::new(store_dyn, sink_dyn)
        .cache(cache)
        .host_router(host_router)
        .analytics_tx(analytics_tx)
        .webhooks(test_webhook_dispatcher())
        .key(KEY)
        .ratelimiter(quark::abuse::ratelimit::RateLimiter::memory(1))
        .public_host(Some("quark.example.com".to_string()))
        .multi_tenant(true)
        .build();
    let app = router(state);

    let mk = |domain: &str| {
        Request::post("/admin/sso-domains")
            .header("content-type", "application/json")
            .header("x-admin-token", &raw)
            .header("cf-connecting-ip", "7.7.7.7")
            .body(Body::from(format!(r#"{{"domain":"{domain}"}}"#)))
            .unwrap()
    };
    let resp1 = app.clone().oneshot(mk("burst-a.com")).await.unwrap();
    assert_eq!(resp1.status(), StatusCode::OK);
    let resp2 = app.oneshot(mk("burst-b.com")).await.unwrap();
    assert_eq!(resp2.status(), StatusCode::TOO_MANY_REQUESTS);

    // The second (rate-limited) call must not have created a row.
    assert_eq!(store.list_sso_domains(tenant).await.unwrap().len(), 1);
}
