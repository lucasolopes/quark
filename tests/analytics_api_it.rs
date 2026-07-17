use axum::body::Body;
use axum::http::{Request, StatusCode};
use quark::api::{router, AppState};
use quark::cache::Cache;
use quark::store::open_backends;
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
