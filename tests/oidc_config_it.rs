use axum::body::Body;
use axum::http::{Request, StatusCode};
use quark::analytics::AnalyticsSink;
use quark::api::{router, AppState};
use quark::auth::{hash_token, ApiToken, Scope};
use quark::cache::Cache;
use quark::dns::NullDns;
use quark::oidc::TenantOidcConfig;
use quark::store::postgres::PostgresStore;
use quark::store::{open_backends, Store};
use quark::tenant::{Tenant, TenantId};
use quark::webhooks::delivery::WebhookDispatcher;
use serial_test::serial;
use std::sync::Arc;
use tower::ServiceExt;

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

// --- Task 2: /admin/oidc-config HTTP endpoints ------------------------------

const KEY: u64 = 0x1234;

fn test_webhook_dispatcher() -> Arc<WebhookDispatcher> {
    let (tx, _rx) = tokio::sync::mpsc::channel(1);
    Arc::new(WebhookDispatcher::new(
        tx,
        Arc::new(std::sync::atomic::AtomicBool::new(false)),
        Arc::new(std::sync::atomic::AtomicBool::new(false)),
    ))
}

/// Builds a router for `tenant`, plus a `x-admin-token` API token with
/// `scopes` scoped to that tenant. `multi_tenant` toggles the cloud gate the
/// three oidc-config endpoints share. Mirrors `invites_it::admin_app_with_scopes`.
async fn admin_app_with_scopes(
    store: Arc<PostgresStore>,
    multi_tenant: bool,
    tenant: TenantId,
    token_id: u64,
    scopes: Vec<Scope>,
) -> (axum::Router, String) {
    let raw = format!("qtok_oidc_config_test_{}", token_id);
    store
        .put_api_token(
            tenant,
            &ApiToken {
                id: token_id,
                name: "oidc-config-test-token".to_string(),
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
    let state = Arc::new(AppState {
        oidc: None,
        sheets: None,
        sheets_api: None,
        oidc_configured: false,
        multi_tenant,
        tenant_domain_suffix: None,
        oidc_tenants: quark::oidc::TenantOidcCache::new(),
        cache,
        store: store_dyn,
        key: KEY,
        signing_key: [0u8; 32],
        analytics_tx,
        sink: sink_dyn,
        admin_token: None,
        ratelimiter: quark::abuse::ratelimit::RateLimiter::disabled(),
        block_private: true,
        public_host: Some("quark.example.com".to_string()),
        real_ip_header: "cf-connecting-ip".to_string(),
        webhooks: test_webhook_dispatcher(),
        host_router,
        dns: Arc::new(NullDns),
    });
    (router(state), raw)
}

fn put_body() -> Body {
    Body::from(
        r#"{
            "issuer": "https://idp.acme.example",
            "client_id": "acme-client",
            "client_secret": "top-secret-value",
            "scopes": ["openid", "profile", "email"],
            "admin_claim": "groups",
            "admin_value": "acme-admins",
            "readonly_value": "acme-viewers",
            "post_login_url": "https://app.acme.example/"
        }"#
        .to_string(),
    )
}

async fn put_oidc_config_http(
    app: &axum::Router,
    token: &str,
    body: Body,
) -> (StatusCode, serde_json::Value, String) {
    let resp = app
        .clone()
        .oneshot(
            Request::put("/admin/oidc-config")
                .header("content-type", "application/json")
                .header("x-admin-token", token)
                .body(body)
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

async fn get_oidc_config_http(
    app: &axum::Router,
    token: &str,
) -> (StatusCode, serde_json::Value, String) {
    let resp = app
        .clone()
        .oneshot(
            Request::get("/admin/oidc-config")
                .header("x-admin-token", token)
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

async fn delete_oidc_config_http(app: &axum::Router, token: &str) -> StatusCode {
    let resp = app
        .clone()
        .oneshot(
            Request::delete("/admin/oidc-config")
                .header("x-admin-token", token)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    resp.status()
}

/// `PUT` as Owner/Admin (Scope::Full) upserts the tenant's config: the store
/// round-trips it, and the stored `client_secret` matches exactly what was
/// PUT (never mutated/hashed at rest, mirroring the Sheets precedent). The
/// PUT's own JSON response never echoes the secret back.
#[tokio::test]
#[serial]
async fn put_oidc_config_http_upserts_and_round_trips_secret() {
    let Some(store) = fresh().await else {
        eprintln!("skip: QUARK_TEST_DATABASE_URL not set");
        return;
    };
    let store = Arc::new(store);
    let tenant = make_tenant(&store, "oidc-cfg-put-a").await;
    let (app, token) =
        admin_app_with_scopes(store.clone(), true, tenant, 9201, vec![Scope::Full]).await;

    let (status, body, raw) = put_oidc_config_http(&app, &token, put_body()).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["issuer"], "https://idp.acme.example");
    assert_eq!(body["client_id"], "acme-client");
    assert_eq!(body["client_secret_set"], true);
    assert!(
        !raw.contains("top-secret-value"),
        "the PUT response must never echo the client_secret back"
    );

    let stored = store
        .get_oidc_config(tenant)
        .await
        .unwrap()
        .expect("config must be stored");
    assert_eq!(stored.tenant_id, tenant);
    assert_eq!(stored.issuer, "https://idp.acme.example");
    assert_eq!(stored.client_id, "acme-client");
    assert_eq!(
        stored.client_secret, "top-secret-value",
        "the stored secret must match exactly what was PUT"
    );
    assert_eq!(stored.scopes, vec!["openid", "profile", "email"]);
    assert_eq!(stored.admin_claim, "groups");
    assert_eq!(stored.admin_value, "acme-admins");
    assert_eq!(stored.readonly_value, "acme-viewers");
    assert_eq!(
        stored.post_login_url,
        Some("https://app.acme.example/".to_string())
    );

    // A second PUT upserts (still one row, updated fields), rather than
    // erroring on the UNIQUE tenant_id.
    let second_body = Body::from(
        r#"{
            "issuer": "https://idp2.acme.example",
            "client_id": "acme-client-2",
            "client_secret": "second-secret",
            "scopes": ["openid"],
            "admin_claim": "roles",
            "admin_value": "admins",
            "readonly_value": "viewers"
        }"#
        .to_string(),
    );
    let (status2, body2, _) = put_oidc_config_http(&app, &token, second_body).await;
    assert_eq!(status2, StatusCode::OK);
    assert_eq!(body2["issuer"], "https://idp2.acme.example");
    let stored2 = store.get_oidc_config(tenant).await.unwrap().unwrap();
    assert_eq!(stored2.issuer, "https://idp2.acme.example");
    assert_eq!(stored2.client_secret, "second-secret");
}

/// `PUT` with an empty (or whitespace-only) `issuer` or `client_id` is
/// rejected: such a config could never drive a real login.
#[tokio::test]
#[serial]
async fn put_oidc_config_http_with_empty_issuer_or_client_id_is_400() {
    let Some(store) = fresh().await else {
        eprintln!("skip: QUARK_TEST_DATABASE_URL not set");
        return;
    };
    let store = Arc::new(store);
    let tenant = make_tenant(&store, "oidc-cfg-put-empty-a").await;
    let (app, token) =
        admin_app_with_scopes(store.clone(), true, tenant, 9202, vec![Scope::Full]).await;

    let empty_issuer = Body::from(
        r#"{"issuer":"","client_id":"c","client_secret":"s","scopes":[],"admin_claim":"g","admin_value":"a","readonly_value":"r"}"#
            .to_string(),
    );
    let (status, _, _) = put_oidc_config_http(&app, &token, empty_issuer).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);

    let empty_client_id = Body::from(
        r#"{"issuer":"https://idp.example","client_id":"   ","client_secret":"s","scopes":[],"admin_claim":"g","admin_value":"a","readonly_value":"r"}"#
            .to_string(),
    );
    let (status2, _, _) = put_oidc_config_http(&app, &token, empty_client_id).await;
    assert_eq!(status2, StatusCode::BAD_REQUEST);

    assert!(store.get_oidc_config(tenant).await.unwrap().is_none());
}

/// `GET` returns the config with `client_secret_set: true`, and the response
/// body never contains the `client_secret` field or its value — the core
/// security assertion for this task.
#[tokio::test]
#[serial]
async fn get_oidc_config_http_redacts_secret() {
    let Some(store) = fresh().await else {
        eprintln!("skip: QUARK_TEST_DATABASE_URL not set");
        return;
    };
    let store = Arc::new(store);
    let tenant = make_tenant(&store, "oidc-cfg-get-a").await;
    let (app, token) =
        admin_app_with_scopes(store.clone(), true, tenant, 9203, vec![Scope::Full]).await;
    let (put_status, _, _) = put_oidc_config_http(&app, &token, put_body()).await;
    assert_eq!(put_status, StatusCode::OK);

    let (status, body, raw) = get_oidc_config_http(&app, &token).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["client_secret_set"], true);
    assert_eq!(body["issuer"], "https://idp.acme.example");
    assert!(
        body.get("client_secret").is_none(),
        "the GET response must never include a client_secret field"
    );
    assert!(
        !raw.contains("top-secret-value"),
        "the GET response body must never contain the secret value"
    );
}

/// `GET` with no config on file -> 404.
#[tokio::test]
#[serial]
async fn get_oidc_config_http_without_one_set_is_404() {
    let Some(store) = fresh().await else {
        eprintln!("skip: QUARK_TEST_DATABASE_URL not set");
        return;
    };
    let store = Arc::new(store);
    let tenant = make_tenant(&store, "oidc-cfg-get-missing-a").await;
    let (app, token) =
        admin_app_with_scopes(store.clone(), true, tenant, 9204, vec![Scope::Full]).await;

    let (status, _, _) = get_oidc_config_http(&app, &token).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

/// `DELETE` removes the config: a `GET` afterward 404s, and a second `DELETE`
/// also 404s (nothing left to remove).
#[tokio::test]
#[serial]
async fn delete_oidc_config_http_removes_it() {
    let Some(store) = fresh().await else {
        eprintln!("skip: QUARK_TEST_DATABASE_URL not set");
        return;
    };
    let store = Arc::new(store);
    let tenant = make_tenant(&store, "oidc-cfg-delete-a").await;
    let (app, token) =
        admin_app_with_scopes(store.clone(), true, tenant, 9205, vec![Scope::Full]).await;
    let (put_status, _, _) = put_oidc_config_http(&app, &token, put_body()).await;
    assert_eq!(put_status, StatusCode::OK);

    let status = delete_oidc_config_http(&app, &token).await;
    assert_eq!(status, StatusCode::OK);
    assert!(store.get_oidc_config(tenant).await.unwrap().is_none());

    let (get_status, _, _) = get_oidc_config_http(&app, &token).await;
    assert_eq!(get_status, StatusCode::NOT_FOUND);

    let status2 = delete_oidc_config_http(&app, &token).await;
    assert_eq!(status2, StatusCode::NOT_FOUND);
}

/// A caller with insufficient scope (Viewer-like: links_read + analytics, no
/// `Scope::Full`) is 403 on all three endpoints, and nothing is written.
#[tokio::test]
#[serial]
async fn non_full_caller_is_403_on_all_three_oidc_config_endpoints() {
    let Some(store) = fresh().await else {
        eprintln!("skip: QUARK_TEST_DATABASE_URL not set");
        return;
    };
    let store = Arc::new(store);
    let tenant = make_tenant(&store, "oidc-cfg-403-a").await;
    let (app, token) = admin_app_with_scopes(
        store.clone(),
        true,
        tenant,
        9206,
        vec![Scope::LinksRead, Scope::Analytics],
    )
    .await;

    let (put_status, _, _) = put_oidc_config_http(&app, &token, put_body()).await;
    assert_eq!(put_status, StatusCode::FORBIDDEN);

    let (get_status, _, _) = get_oidc_config_http(&app, &token).await;
    assert_eq!(get_status, StatusCode::FORBIDDEN);

    let delete_status = delete_oidc_config_http(&app, &token).await;
    assert_eq!(delete_status, StatusCode::FORBIDDEN);

    assert!(store.get_oidc_config(tenant).await.unwrap().is_none());
}

/// All three `/admin/oidc-config` surfaces 404 when `multi_tenant = false`,
/// with no Postgres configured at all and no credential presented — the flag
/// gate runs before authentication. Mirrors
/// `invites_it::oss_invites_endpoints_are_404_without_postgres`.
#[tokio::test]
async fn oidc_config_endpoints_404_in_oss_without_postgres() {
    let dir = tempfile::tempdir().unwrap();
    let (store, sink) = open_backends(dir.path(), false).await.unwrap();
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
        multi_tenant: false,
        tenant_domain_suffix: None,
        oidc_tenants: quark::oidc::TenantOidcCache::new(),
        cache,
        store,
        key: KEY,
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
        dns: Arc::new(NullDns),
    });
    let app = router(state);

    let (status, _, _) = put_oidc_config_http(&app, "whatever", put_body()).await;
    assert_eq!(status, StatusCode::NOT_FOUND);

    let (status, _, _) = get_oidc_config_http(&app, "whatever").await;
    assert_eq!(status, StatusCode::NOT_FOUND);

    let status = delete_oidc_config_http(&app, "whatever").await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}
