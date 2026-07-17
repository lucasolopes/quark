use axum::body::Body;
use axum::http::{Request, StatusCode};
use quark::analytics::{AnalyticsSink, ClickEvent};
use quark::api::{router, AppState};
use quark::auth::{hash_token, ApiToken, Scope};
use quark::cache::Cache;
use quark::store::{open_backends, postgres::PostgresStore, Record, Store};
use quark::tenant::TenantId;
use serial_test::serial;
use std::sync::Arc;
use tower::ServiceExt;

async fn app_with(
    admin: Option<&str>,
    chan_cap: usize,
) -> (
    axum::Router,
    tokio::sync::mpsc::Receiver<quark::analytics::ClickEvent>,
) {
    let dir = Box::leak(Box::new(tempfile::tempdir().unwrap()));
    let (store, sink) = open_backends(dir.path(), false).await.unwrap();
    let cache = Cache::new(store.clone(), 1000, None);
    let host_router = Arc::new(quark::domain_router::HostRouter::new(
        store.clone(),
        None,
        None,
    ));
    let (tx, rx) = tokio::sync::mpsc::channel(chan_cap);
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
        key: 0x1234,
        signing_key: [0u8; 32],
        analytics_tx: tx,
        sink,
        admin_token: admin.map(|s| s.to_string()),
        ratelimiter: quark::abuse::ratelimit::RateLimiter::disabled(),
        block_private: true,
        public_host: None,
        real_ip_header: "cf-connecting-ip".to_string(),
        webhooks: test_webhook_dispatcher(),
        host_router,
        dns: std::sync::Arc::new(quark::dns::NullDns),
    });
    (router(state), rx)
}

/// A `WebhookDispatcher` for tests that don't exercise webhooks: the
/// receiver is dropped immediately, so `emit` silently no-ops (logs and
/// drops) rather than needing a live worker.
fn test_webhook_dispatcher() -> Arc<quark::webhooks::delivery::WebhookDispatcher> {
    let (tx, _rx) = tokio::sync::mpsc::channel(1);
    Arc::new(quark::webhooks::delivery::WebhookDispatcher::new(
        tx,
        Arc::new(std::sync::atomic::AtomicBool::new(false)),
        Arc::new(std::sync::atomic::AtomicBool::new(false)),
    ))
}

async fn create(app: &axum::Router, url: &str, token: Option<&str>) -> String {
    let mut req = Request::post("/").header("content-type", "application/json");
    if let Some(t) = token {
        req = req.header("x-admin-token", t);
    }
    let resp = app
        .clone()
        .oneshot(
            req.body(Body::from(format!(r#"{{"url":"{url}"}}"#)))
                .unwrap(),
        )
        .await
        .unwrap();
    let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .unwrap();
    let v: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    v["code"].as_str().unwrap().to_string()
}

#[tokio::test]
async fn redirect_does_not_block_when_queue_is_full() {
    let (app, _rx) = app_with(None, 1).await;
    let code = create(&app, "https://example.com", None).await;
    for _ in 0..5 {
        let resp = app
            .clone()
            .oneshot(
                Request::get(format!("/{code}"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::FOUND);
    }
}

#[tokio::test]
async fn stats_requires_token() {
    let (app, _rx) = app_with(Some("secret"), 100).await;
    let code = create(&app, "https://example.com", Some("secret")).await;
    let resp = app
        .clone()
        .oneshot(
            Request::get(format!("/{code}/stats"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
    let resp = app
        .clone()
        .oneshot(
            Request::get(format!("/{code}/stats"))
                .header("x-admin-token", "wrong")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
    let resp = app
        .clone()
        .oneshot(
            Request::get(format!("/{code}/stats"))
                .header("x-admin-token", "secret")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .unwrap();
    let v: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    assert!(v["aggregates"].is_object());
    assert!(v["recent"].is_array());
}

#[tokio::test]
async fn stats_404_nonexistent_code() {
    let (app, _rx) = app_with(Some("secret"), 100).await;
    let resp = app
        .clone()
        .oneshot(
            Request::get("/0000000/stats")
                .header("x-admin-token", "secret")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn stats_disabled_without_configured_token() {
    let (app, _rx) = app_with(None, 100).await;
    let code = create(&app, "https://example.com", None).await;
    let resp = app
        .clone()
        .oneshot(
            Request::get(format!("/{code}/stats"))
                .header("x-admin-token", "anything")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
}

/// PG-gated: `GET /admin/stats` (multi-tenancy P4a Task 2). Builds a full
/// router directly over a `PostgresStore` (both `Store` and `AnalyticsSink`),
/// since the LMDB-backed `app_with` above can't isolate by tenant (see
/// `analytics_sink_it.rs`'s `stats_for_tenant_non_default_tenant_is_empty...`
/// test for that backend's honest single-tenant limits).
async fn fresh_pg() -> Option<PostgresStore> {
    let url = std::env::var("QUARK_TEST_DATABASE_URL").ok()?;
    let s = PostgresStore::open(&url, true).await.unwrap();
    s.reset_for_tests().await.unwrap();
    Some(s)
}

fn rec_for(tenant: TenantId, url: &str) -> Record {
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
        tenant_id: tenant,
    }
}

fn ev_for(id: u64, ts: u64, tenant: TenantId, country: &str) -> ClickEvent {
    ClickEvent {
        id,
        event_id: String::new(),
        ts,
        referer: None,
        country: Some(country.into()),
        user_agent: Some("Mozilla/5.0 (iPhone)".into()),
        city: None,
        bot: false,
        ip: None,
        fbc: None,
        variant: None,
        tenant_id: tenant.0,
    }
}

fn app_over_pg(store: Arc<PostgresStore>) -> axum::Router {
    let cache = Cache::new(store.clone() as Arc<dyn Store>, 1000, None);
    let host_router = Arc::new(quark::domain_router::HostRouter::new(
        store.clone() as Arc<dyn Store>,
        None,
        None,
    ));
    let (analytics_tx, _rx) = tokio::sync::mpsc::channel(100);
    let (wtx, _wrx) = tokio::sync::mpsc::channel(1);
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
        store: store.clone() as Arc<dyn Store>,
        key: 0x1234,
        signing_key: [0u8; 32],
        analytics_tx,
        sink: store as Arc<dyn AnalyticsSink>,
        admin_token: None,
        ratelimiter: quark::abuse::ratelimit::RateLimiter::disabled(),
        block_private: true,
        public_host: None,
        real_ip_header: "cf-connecting-ip".to_string(),
        webhooks: Arc::new(quark::webhooks::delivery::WebhookDispatcher::new(
            wtx,
            Arc::new(std::sync::atomic::AtomicBool::new(false)),
            Arc::new(std::sync::atomic::AtomicBool::new(false)),
        )),
        host_router,
        dns: std::sync::Arc::new(quark::dns::NullDns),
    });
    router(state)
}

/// A caller holding a Scope::Analytics token scoped to tenant B must get
/// ONLY tenant B's aggregate from `/admin/stats` — never tenant A's clicks,
/// even though both tenants recorded events in the same tables. This is the
/// HTTP-level half of the isolation contract (`stats_for_tenant_isolates_...`
/// in `postgres_analytics_it.rs` covers the sink level directly).
#[tokio::test]
#[serial(pg)]
async fn admin_stats_isolates_tenant_b_from_tenant_a() {
    let Some(store) = fresh_pg().await else {
        eprintln!("skip: QUARK_TEST_DATABASE_URL not set");
        return;
    };
    let tenant_a = TenantId(30);
    let tenant_b = TenantId(31);
    store
        .put_link(tenant_a, 500, &rec_for(tenant_a, "https://example.com/a"))
        .await
        .unwrap();
    store
        .put_link(tenant_b, 501, &rec_for(tenant_b, "https://example.com/b"))
        .await
        .unwrap();
    store
        .record_batch(&[
            ev_for(500, 1_752_300_000, tenant_a, "BR"),
            ev_for(500, 1_752_300_050, tenant_a, "BR"),
        ])
        .await
        .unwrap();
    store
        .record_batch(&[ev_for(501, 1_752_300_100, tenant_b, "JP")])
        .await
        .unwrap();

    let plaintext = "qtok_admin_stats_tenant_b";
    let token = ApiToken {
        id: 1,
        name: "b-token".into(),
        token_hash: hash_token(plaintext),
        scopes: vec![Scope::Analytics],
        rate_limit_per_min: None,
        created: 0,
        tenant_id: tenant_b,
    };
    store.put_api_token(tenant_b, &token).await.unwrap();

    let app = app_over_pg(Arc::new(store));
    let resp = app
        .oneshot(
            Request::get("/admin/stats")
                .header("x-admin-token", plaintext)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .unwrap();
    let agg: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(
        agg["total"], 1,
        "tenant B's endpoint must count only B's click"
    );
    assert_eq!(agg["per_country"]["JP"], 1);
    assert!(
        agg["per_country"].get("BR").is_none(),
        "tenant B's /admin/stats must never surface tenant A's country"
    );
}
