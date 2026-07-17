use axum::body::Body;
use axum::http::{Request, StatusCode};
use base64::{engine::general_purpose::STANDARD as b64, Engine as _};
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
use sqlx::postgres::PgPoolOptions;
use sqlx::{PgPool, Row};
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
        required_value: None,
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

/// `required_value` (multi-tenancy P2d Task 4b) round-trips through the
/// JSONB `blob` exactly like every other field, both set and unset (the
/// `#[serde(default)]` compatibility path for blobs written before the field
/// existed is exercised in `store::postgres` unit-style via deserialization,
/// but the round trip through a real Postgres write/read is asserted here).
#[tokio::test]
#[serial]
async fn required_value_round_trips_set_and_unset() {
    let Some(store) = fresh().await else {
        eprintln!("skip: QUARK_TEST_DATABASE_URL not set");
        return;
    };
    let a = make_tenant(&store, "oidc-required-a").await;
    let mut with_required = cfg(a, "https://idp.acme.example");
    with_required.required_value = Some("acme-contractors".to_string());
    store.put_oidc_config(&with_required).await.unwrap();
    let got = store.get_oidc_config(a).await.unwrap().unwrap();
    assert_eq!(got.required_value, Some("acme-contractors".to_string()));

    let b = make_tenant(&store, "oidc-required-b").await;
    let without_required = cfg(b, "https://idp.acme.example");
    assert_eq!(without_required.required_value, None);
    store.put_oidc_config(&without_required).await.unwrap();
    let got_b = store.get_oidc_config(b).await.unwrap().unwrap();
    assert_eq!(got_b.required_value, None);
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
        keycloak: None,
        keycloak_base_url: None,
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

/// `required_value` (multi-tenancy P2d Task 4b) is accepted on `PUT` and
/// reflected on `GET` — it is not secret, so unlike `client_secret` it rides
/// in the redacted view unchanged.
#[tokio::test]
#[serial]
async fn required_value_accepted_on_put_and_reflected_on_get() {
    let Some(store) = fresh().await else {
        eprintln!("skip: QUARK_TEST_DATABASE_URL not set");
        return;
    };
    let store = Arc::new(store);
    let tenant = make_tenant(&store, "oidc-cfg-required-http-a").await;
    let (app, token) =
        admin_app_with_scopes(store.clone(), true, tenant, 9207, vec![Scope::Full]).await;

    let body = Body::from(
        r#"{
            "issuer": "https://idp.acme.example",
            "client_id": "acme-client",
            "client_secret": "top-secret-value",
            "scopes": ["openid"],
            "admin_claim": "groups",
            "admin_value": "acme-admins",
            "readonly_value": "acme-viewers",
            "required_value": "acme-contractors"
        }"#
        .to_string(),
    );
    let (status, put_body, _) = put_oidc_config_http(&app, &token, body).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(put_body["required_value"], "acme-contractors");

    let (get_status, get_body, _) = get_oidc_config_http(&app, &token).await;
    assert_eq!(get_status, StatusCode::OK);
    assert_eq!(get_body["required_value"], "acme-contractors");

    let stored = store.get_oidc_config(tenant).await.unwrap().unwrap();
    assert_eq!(stored.required_value, Some("acme-contractors".to_string()));
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
#[serial]
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
        keycloak: None,
        keycloak_base_url: None,
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

/// `GET /admin/login` and `GET /admin/callback` are `404` too, without
/// Postgres and without any global env OIDC configured — the same ungated
/// OSS shape as `oidc_config_endpoints_404_in_oss_without_postgres`, but for
/// the login/callback surface rather than the CRUD one. This is the router
/// path (not the direct-handler-call path the `api.rs` unit tests use), so it
/// proves the actual routes wired into `router()` carry the gate, not just
/// the functions behind them.
#[tokio::test]
#[serial]
async fn oss_login_and_callback_404_without_oidc_configured_or_postgres() {
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
        oidc_configured: false,
        multi_tenant: false,
        tenant_domain_suffix: None,
        oidc_tenants: quark::oidc::TenantOidcCache::new(),
        keycloak: None,
        keycloak_base_url: None,
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

    let login_resp = app
        .clone()
        .oneshot(Request::get("/admin/login").body(Body::empty()).unwrap())
        .await
        .unwrap();
    assert_eq!(login_resp.status(), StatusCode::NOT_FOUND);

    let callback_resp = app
        .clone()
        .oneshot(
            Request::get("/admin/callback?code=c&state=s")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(callback_resp.status(), StatusCode::NOT_FOUND);
}

/// A `?org=` login against an OSS deployment (`multi_tenant: false`) is also
/// `404`, at the router level, mirroring `org_login_requires_multi_tenant_mode`
/// in the `api.rs` unit tests (which calls the handler directly) but through
/// the actual route.
#[tokio::test]
#[serial]
async fn oss_org_login_404_at_router_level() {
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
        oidc_configured: false,
        multi_tenant: false,
        tenant_domain_suffix: None,
        oidc_tenants: quark::oidc::TenantOidcCache::new(),
        keycloak: None,
        keycloak_base_url: None,
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

    let resp = app
        .oneshot(
            Request::get("/admin/login?org=acme")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
}

// --- Task 5: security sweep -------------------------------------------------

/// Builds a router with an env break-glass admin token, `multi_tenant: true`.
/// Used to prove break-glass authorization is unaffected by per-tenant OIDC
/// configs existing for other tenants.
async fn admin_app_with_breakglass_token(store: Arc<PostgresStore>, token: &str) -> axum::Router {
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
        multi_tenant: true,
        tenant_domain_suffix: None,
        oidc_tenants: quark::oidc::TenantOidcCache::new(),
        keycloak: None,
        keycloak_base_url: None,
        cache,
        store: store_dyn,
        key: KEY,
        signing_key: [0u8; 32],
        analytics_tx,
        sink: sink_dyn,
        admin_token: Some(token.to_string()),
        ratelimiter: quark::abuse::ratelimit::RateLimiter::disabled(),
        block_private: true,
        public_host: Some("quark.example.com".to_string()),
        real_ip_header: "cf-connecting-ip".to_string(),
        webhooks: test_webhook_dispatcher(),
        host_router,
        dns: Arc::new(NullDns),
    });
    router(state)
}

/// A tenant B token (`Scope::Full`, scoped to B) can never see, overwrite, or
/// remove tenant A's OIDC config through `/admin/oidc-config`: `GET` 404s
/// (not A's config), `PUT` creates B's own row without touching A's, and
/// `DELETE` (before B has a config) 404s — all while A's config, verified via
/// the store directly, is untouched throughout. This is the HTTP-level
/// counterpart to the store-level `tenant_scoped_read_does_not_leak_across_tenants`:
/// it proves the isolation holds through `admin_guard`'s `Principal.tenant`,
/// not just through the store's `WHERE tenant_id` predicate.
#[tokio::test]
#[serial]
async fn cross_tenant_token_cannot_see_edit_or_delete_another_tenants_config() {
    let Some(store) = fresh().await else {
        eprintln!("skip: QUARK_TEST_DATABASE_URL not set");
        return;
    };
    let store = Arc::new(store);
    let tenant_a = make_tenant(&store, "oidc-xtenant-a").await;
    let tenant_b = make_tenant(&store, "oidc-xtenant-b").await;
    let config_a = cfg(tenant_a, "https://idp.acme.example");
    store.put_oidc_config(&config_a).await.unwrap();

    let (app_b, token_b) =
        admin_app_with_scopes(store.clone(), true, tenant_b, 9301, vec![Scope::Full]).await;

    // B's GET never sees A's config.
    let (get_status, get_body, get_raw) = get_oidc_config_http(&app_b, &token_b).await;
    assert_eq!(get_status, StatusCode::NOT_FOUND);
    assert_eq!(get_body, serde_json::Value::Null);
    assert!(!get_raw.contains("idp.acme.example"));

    // B's DELETE (no config of its own yet) 404s rather than removing A's row.
    let delete_status = delete_oidc_config_http(&app_b, &token_b).await;
    assert_eq!(delete_status, StatusCode::NOT_FOUND);
    assert_eq!(
        store.get_oidc_config(tenant_a).await.unwrap().as_ref(),
        Some(&config_a),
        "tenant A's config must survive tenant B's DELETE"
    );

    // B's PUT creates B's OWN row; A's config is untouched.
    let (put_status, put_body, _) = put_oidc_config_http(&app_b, &token_b, put_body()).await;
    assert_eq!(put_status, StatusCode::OK);
    assert_eq!(put_body["issuer"], "https://idp.acme.example"); // same fixture body, different tenant row
    let stored_a = store.get_oidc_config(tenant_a).await.unwrap().unwrap();
    assert_eq!(
        stored_a, config_a,
        "tenant A's config must be unchanged after tenant B's PUT"
    );
    let stored_b = store.get_oidc_config(tenant_b).await.unwrap().unwrap();
    assert_eq!(stored_b.tenant_id, tenant_b);

    // B's own subsequent GET now sees B's row, still never A's.
    let (get_status2, get_body2, _) = get_oidc_config_http(&app_b, &token_b).await;
    assert_eq!(get_status2, StatusCode::OK);
    assert_eq!(get_body2["client_id"], "acme-client");
}

/// The env break-glass admin token still authorizes `Scope::Full` at tenant 0
/// (`DEFAULT_TENANT`) exactly as before, even when some OTHER tenant has its
/// own per-tenant OIDC config on file. Driven through `GET
/// /admin/oidc-config` itself: the break-glass token resolves a `Principal`
/// at `DEFAULT_TENANT` regardless of OIDC-per-tenant state elsewhere, so it
/// reaches the "no config for tenant 0" branch (`404`) rather than being
/// rejected by `admin_guard` (`401`/`403`) — proof the authorization step
/// itself is unaffected.
#[tokio::test]
#[serial]
async fn breakglass_admin_token_unaffected_by_other_tenants_oidc_config() {
    let Some(store) = fresh().await else {
        eprintln!("skip: QUARK_TEST_DATABASE_URL not set");
        return;
    };
    let store = Arc::new(store);
    let configured_tenant = make_tenant(&store, "oidc-breakglass-configured").await;
    store
        .put_oidc_config(&cfg(configured_tenant, "https://idp.acme.example"))
        .await
        .unwrap();

    let app = admin_app_with_breakglass_token(store.clone(), "breakglass-secret").await;
    let (status, _, _) = get_oidc_config_http(&app, "breakglass-secret").await;
    // DEFAULT_TENANT (0) has no config of its own: reaching 404 here (not
    // 401/403) proves admin_guard authorized the break-glass token at
    // DEFAULT_TENANT with Scope::Full, unaffected by `configured_tenant`'s
    // own per-tenant OIDC config existing.
    assert_eq!(status, StatusCode::NOT_FOUND);

    // Sanity: a wrong token is still rejected (401), so the 404 above is not
    // just "auth is a no-op in this test wiring".
    let (wrong_status, _, _) = get_oidc_config_http(&app, "not-the-right-token").await;
    assert_eq!(wrong_status, StatusCode::UNAUTHORIZED);

    // And the OTHER tenant's config is completely undisturbed.
    let still_there = store.get_oidc_config(configured_tenant).await.unwrap();
    assert!(still_there.is_some());
}

// --- LUC-48 Task 2: encrypt oidc client_secret at rest ----------------------

/// A valid 32-byte key, base64-encoded, for `QUARK_ENCRYPTION_KEY`.
fn test_key() -> String {
    b64.encode([9u8; 32])
}

/// A fresh store plus a raw pool used to inspect the `oidc_configs.blob` and
/// `sheets_connection.blob` JSONB columns directly (the `Store` trait never
/// exposes raw column contents).
async fn fresh_with_pool() -> Option<(PostgresStore, PgPool)> {
    let url = std::env::var("QUARK_TEST_DATABASE_URL").ok()?;
    let s = PostgresStore::open(&url, true).await.unwrap();
    s.reset_for_tests().await.unwrap();
    let pool = PgPoolOptions::new()
        .max_connections(4)
        .connect(&url)
        .await
        .unwrap();
    Some((s, pool))
}

async fn fresh_with_pool_and_key() -> Option<(PostgresStore, PgPool)> {
    let url = std::env::var("QUARK_TEST_DATABASE_URL").ok()?;
    std::env::set_var("QUARK_ENCRYPTION_KEY", test_key());
    let s = PostgresStore::open(&url, true).await.unwrap();
    std::env::remove_var("QUARK_ENCRYPTION_KEY");
    s.reset_for_tests().await.unwrap();
    let pool = PgPoolOptions::new()
        .max_connections(4)
        .connect(&url)
        .await
        .unwrap();
    Some((s, pool))
}

async fn raw_oidc_client_secret(pool: &PgPool, tenant: TenantId) -> String {
    let row =
        sqlx::query("SELECT blob->>'client_secret' AS s FROM oidc_configs WHERE tenant_id = $1")
            .bind(tenant.0 as i64)
            .fetch_one(pool)
            .await
            .unwrap();
    row.try_get("s").unwrap()
}

/// With `QUARK_ENCRYPTION_KEY` set at `open`, `put_oidc_config` stores the
/// `client_secret` sealed (`enc:v1:` prefix) in the raw JSONB column, while
/// `get_oidc_config_bare` (and the tenant-scoped `get_oidc_config`) still
/// return the original plaintext to callers.
#[tokio::test]
#[serial]
async fn client_secret_is_sealed_at_rest_when_key_is_set() {
    let Some((store, pool)) = fresh_with_pool_and_key().await else {
        eprintln!("skip: QUARK_TEST_DATABASE_URL not set");
        return;
    };
    let a = make_tenant(&store, "oidc-enc-a").await;
    let config = cfg(a, "https://idp.acme.example");
    store.put_oidc_config(&config).await.unwrap();

    let raw = raw_oidc_client_secret(&pool, a).await;
    assert!(
        raw.starts_with("enc:v1:"),
        "raw client_secret column must be sealed, got: {raw}"
    );

    let bare = store.get_oidc_config_bare(a).await.unwrap().unwrap();
    assert_eq!(bare.client_secret, config.client_secret);
    let scoped = store.get_oidc_config(a).await.unwrap().unwrap();
    assert_eq!(scoped.client_secret, config.client_secret);
}

/// Without `QUARK_ENCRYPTION_KEY` (today's default), the raw `client_secret`
/// column stays plaintext — parity with pre-LUC-48 behavior.
#[tokio::test]
#[serial]
async fn client_secret_stays_plaintext_at_rest_when_key_is_unset() {
    let Some((store, pool)) = fresh_with_pool().await else {
        eprintln!("skip: QUARK_TEST_DATABASE_URL not set");
        return;
    };
    let a = make_tenant(&store, "oidc-plain-a").await;
    let config = cfg(a, "https://idp.acme.example");
    store.put_oidc_config(&config).await.unwrap();

    let raw = raw_oidc_client_secret(&pool, a).await;
    assert_eq!(raw, config.client_secret);
}

/// Legacy passthrough: a config written with no key set (plaintext secret) is
/// still readable once encryption is turned on (`get_oidc_config_bare` with
/// the key present returns the original plaintext unchanged). Re-putting it
/// with the key now set upgrades the raw column to `enc:v1:`.
#[tokio::test]
#[serial]
async fn legacy_plaintext_client_secret_is_read_then_upgraded_on_repeat_write() {
    let Some(url) = std::env::var("QUARK_TEST_DATABASE_URL").ok() else {
        eprintln!("skip: QUARK_TEST_DATABASE_URL not set");
        return;
    };

    // Phase 1: no key. Write a plaintext secret.
    let store1 = PostgresStore::open(&url, true).await.unwrap();
    store1.reset_for_tests().await.unwrap();
    let a = make_tenant(&store1, "oidc-legacy-a").await;
    let config = cfg(a, "https://idp.acme.example");
    store1.put_oidc_config(&config).await.unwrap();

    // Phase 2: a NEW store opened WITH the key. The legacy plaintext row is
    // still readable, unchanged.
    std::env::set_var("QUARK_ENCRYPTION_KEY", test_key());
    let store2 = PostgresStore::open(&url, true).await.unwrap();
    std::env::remove_var("QUARK_ENCRYPTION_KEY");
    let bare = store2.get_oidc_config_bare(a).await.unwrap().unwrap();
    assert_eq!(bare.client_secret, config.client_secret);

    // Re-putting (key still active for store2) upgrades the raw column.
    store2.put_oidc_config(&config).await.unwrap();
    let pool = PgPoolOptions::new()
        .max_connections(2)
        .connect(&url)
        .await
        .unwrap();
    let raw = raw_oidc_client_secret(&pool, a).await;
    assert!(
        raw.starts_with("enc:v1:"),
        "raw client_secret column must be sealed after the re-put, got: {raw}"
    );
}

// --- LUC-48 Task 2: encrypt sheets refresh_token at rest --------------------

fn sheets_connection(refresh_token: &str) -> quark::sheets::SheetsConnection {
    quark::sheets::SheetsConnection {
        refresh_token: refresh_token.to_string(),
        email: "me@acme.example".to_string(),
        spreadsheet_id: Some("sheet-1".to_string()),
        last_sync: None,
        last_status: quark::sheets::SyncStatus::Never,
    }
}

async fn raw_sheets_refresh_token(pool: &PgPool, tenant: TenantId) -> String {
    let row = sqlx::query(
        "SELECT blob->>'refresh_token' AS s FROM sheets_connection WHERE tenant_id = $1",
    )
    .bind(tenant.0 as i64)
    .fetch_one(pool)
    .await
    .unwrap();
    row.try_get("s").unwrap()
}

/// With the key set, `put_sheets_connection` seals `refresh_token` at rest,
/// and `get_sheets_connection` still returns the original plaintext.
#[tokio::test]
#[serial]
async fn sheets_refresh_token_is_sealed_at_rest_when_key_is_set() {
    let Some((store, pool)) = fresh_with_pool_and_key().await else {
        eprintln!("skip: QUARK_TEST_DATABASE_URL not set");
        return;
    };
    let a = make_tenant(&store, "sheets-enc-a").await;
    let conn = sheets_connection("refresh-tok-abc123");
    store.put_sheets_connection(a, &conn).await.unwrap();

    let raw = raw_sheets_refresh_token(&pool, a).await;
    assert!(
        raw.starts_with("enc:v1:"),
        "raw refresh_token column must be sealed, got: {raw}"
    );

    let got = store.get_sheets_connection(a).await.unwrap().unwrap();
    assert_eq!(got.refresh_token, conn.refresh_token);
}

/// Without a key, the raw `refresh_token` column stays plaintext — parity
/// with today's behavior.
#[tokio::test]
#[serial]
async fn sheets_refresh_token_stays_plaintext_at_rest_when_key_is_unset() {
    let Some((store, pool)) = fresh_with_pool().await else {
        eprintln!("skip: QUARK_TEST_DATABASE_URL not set");
        return;
    };
    let a = make_tenant(&store, "sheets-plain-a").await;
    let conn = sheets_connection("refresh-tok-xyz789");
    store.put_sheets_connection(a, &conn).await.unwrap();

    let raw = raw_sheets_refresh_token(&pool, a).await;
    assert_eq!(raw, conn.refresh_token);
}

/// Legacy passthrough for sheets: a connection written with no key set reads
/// back fine once the key is turned on, and a re-put upgrades the raw column.
#[tokio::test]
#[serial]
async fn legacy_plaintext_refresh_token_is_read_then_upgraded_on_repeat_write() {
    let Some(url) = std::env::var("QUARK_TEST_DATABASE_URL").ok() else {
        eprintln!("skip: QUARK_TEST_DATABASE_URL not set");
        return;
    };

    let store1 = PostgresStore::open(&url, true).await.unwrap();
    store1.reset_for_tests().await.unwrap();
    let a = make_tenant(&store1, "sheets-legacy-a").await;
    let conn = sheets_connection("refresh-tok-legacy");
    store1.put_sheets_connection(a, &conn).await.unwrap();

    std::env::set_var("QUARK_ENCRYPTION_KEY", test_key());
    let store2 = PostgresStore::open(&url, true).await.unwrap();
    std::env::remove_var("QUARK_ENCRYPTION_KEY");
    let got = store2.get_sheets_connection(a).await.unwrap().unwrap();
    assert_eq!(got.refresh_token, conn.refresh_token);

    store2.put_sheets_connection(a, &conn).await.unwrap();
    let pool = PgPoolOptions::new()
        .max_connections(2)
        .connect(&url)
        .await
        .unwrap();
    let raw = raw_sheets_refresh_token(&pool, a).await;
    assert!(
        raw.starts_with("enc:v1:"),
        "raw refresh_token column must be sealed after the re-put, got: {raw}"
    );
}

// --- LUC-48 Task 3: boot backfill re-encrypts legacy plaintext secrets -----

/// A legacy `client_secret` (written before the key existed) is found and
/// sealed by `reencrypt_legacy_secrets` once a store is opened with the key;
/// the decrypted value is unchanged, and a second pass finds nothing left to
/// do (idempotent).
#[tokio::test]
#[serial]
async fn backfill_reencrypts_legacy_oidc_client_secret_and_is_idempotent() {
    let Some(url) = std::env::var("QUARK_TEST_DATABASE_URL").ok() else {
        eprintln!("skip: QUARK_TEST_DATABASE_URL not set");
        return;
    };

    // Phase 1: no key. Write a plaintext secret.
    let store1 = PostgresStore::open(&url, true).await.unwrap();
    store1.reset_for_tests().await.unwrap();
    let a = make_tenant(&store1, "oidc-backfill-a").await;
    let config = cfg(a, "https://idp.acme.example");
    store1.put_oidc_config(&config).await.unwrap();

    // Phase 2: a NEW store opened WITH the key.
    std::env::set_var("QUARK_ENCRYPTION_KEY", test_key());
    let store2 = PostgresStore::open(&url, true).await.unwrap();
    std::env::remove_var("QUARK_ENCRYPTION_KEY");

    let n = store2.reencrypt_legacy_secrets().await.unwrap();
    assert_eq!(n, 1, "exactly the one legacy row must be re-encrypted");

    let pool = PgPoolOptions::new()
        .max_connections(2)
        .connect(&url)
        .await
        .unwrap();
    let raw = raw_oidc_client_secret(&pool, a).await;
    assert!(
        raw.starts_with("enc:v1:"),
        "raw client_secret column must be sealed after the backfill, got: {raw}"
    );
    let got = store2.get_oidc_config_bare(a).await.unwrap().unwrap();
    assert_eq!(got.client_secret, config.client_secret);

    // Running again finds nothing left to re-encrypt.
    let n2 = store2.reencrypt_legacy_secrets().await.unwrap();
    assert_eq!(n2, 0, "already-sealed rows must be skipped (idempotent)");
}

/// Same shape as the oidc case above, for the sheets `refresh_token`.
#[tokio::test]
#[serial]
async fn backfill_reencrypts_legacy_sheets_refresh_token_and_is_idempotent() {
    let Some(url) = std::env::var("QUARK_TEST_DATABASE_URL").ok() else {
        eprintln!("skip: QUARK_TEST_DATABASE_URL not set");
        return;
    };

    let store1 = PostgresStore::open(&url, true).await.unwrap();
    store1.reset_for_tests().await.unwrap();
    let a = make_tenant(&store1, "sheets-backfill-a").await;
    let conn = sheets_connection("refresh-tok-backfill");
    store1.put_sheets_connection(a, &conn).await.unwrap();

    std::env::set_var("QUARK_ENCRYPTION_KEY", test_key());
    let store2 = PostgresStore::open(&url, true).await.unwrap();
    std::env::remove_var("QUARK_ENCRYPTION_KEY");

    let n = store2.reencrypt_legacy_secrets().await.unwrap();
    assert_eq!(n, 1, "exactly the one legacy row must be re-encrypted");

    let pool = PgPoolOptions::new()
        .max_connections(2)
        .connect(&url)
        .await
        .unwrap();
    let raw = raw_sheets_refresh_token(&pool, a).await;
    assert!(
        raw.starts_with("enc:v1:"),
        "raw refresh_token column must be sealed after the backfill, got: {raw}"
    );
    let got = store2.get_sheets_connection(a).await.unwrap().unwrap();
    assert_eq!(got.refresh_token, conn.refresh_token);

    let n2 = store2.reencrypt_legacy_secrets().await.unwrap();
    assert_eq!(n2, 0, "already-sealed rows must be skipped (idempotent)");
}

/// Without a key (`secretbox` is `None`), the backfill is a no-op: returns 0
/// and leaves the plaintext column exactly as it was.
#[tokio::test]
#[serial]
async fn backfill_is_noop_without_a_key() {
    let Some((store, pool)) = fresh_with_pool().await else {
        eprintln!("skip: QUARK_TEST_DATABASE_URL not set");
        return;
    };
    let a = make_tenant(&store, "oidc-backfill-nokey-a").await;
    let config = cfg(a, "https://idp.acme.example");
    store.put_oidc_config(&config).await.unwrap();

    let n = store.reencrypt_legacy_secrets().await.unwrap();
    assert_eq!(n, 0, "no key configured means nothing to backfill");

    let raw = raw_oidc_client_secret(&pool, a).await;
    assert_eq!(raw, config.client_secret);
}
