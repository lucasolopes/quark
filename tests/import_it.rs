use axum::body::Body;
use axum::http::{Request, StatusCode};
use quark::api::router;
use quark::cache::Cache;
use quark::store::open_backends;
use std::sync::Arc;
use tower::ServiceExt;

mod common;

/// A `WebhookDispatcher` for tests that don't exercise webhooks: the
/// receiver is dropped immediately, so `emit` silently no-ops.
fn test_webhook_dispatcher() -> Arc<quark::webhooks::delivery::WebhookDispatcher> {
    let (tx, _rx) = tokio::sync::mpsc::channel(1);
    Arc::new(quark::webhooks::delivery::WebhookDispatcher::new(
        tx,
        Arc::new(std::sync::atomic::AtomicBool::new(false)),
        Arc::new(std::sync::atomic::AtomicBool::new(false)),
    ))
}

async fn app_admin(token: &str) -> axum::Router {
    let dir = Box::leak(Box::new(tempfile::tempdir().unwrap()));
    let (store, sink) = open_backends(dir.path(), false).await.unwrap();
    let cache = Cache::new(store.clone(), 1000, None);
    let host_router = Arc::new(quark::domain_router::HostRouter::new(
        store.clone(),
        None,
        None,
    ));
    let (tx, _rx) = tokio::sync::mpsc::channel(100);
    let state = common::TestState::new(store, sink)
        .cache(cache)
        .host_router(host_router)
        .analytics_tx(tx)
        .webhooks(test_webhook_dispatcher())
        .admin_token(Some(token.to_string()))
        .build();
    router(state)
}

async fn app_no_admin() -> axum::Router {
    let dir = Box::leak(Box::new(tempfile::tempdir().unwrap()));
    let (store, sink) = open_backends(dir.path(), false).await.unwrap();
    let cache = Cache::new(store.clone(), 1000, None);
    let host_router = Arc::new(quark::domain_router::HostRouter::new(
        store.clone(),
        None,
        None,
    ));
    let (tx, _rx) = tokio::sync::mpsc::channel(100);
    let state = common::TestState::new(store, sink)
        .cache(cache)
        .host_router(host_router)
        .analytics_tx(tx)
        .webhooks(test_webhook_dispatcher())
        .build();
    router(state)
}

#[tokio::test]
async fn import_json_creates_n_links_resolvable() {
    let app = app_admin("secret").await;
    let body = r#"[{"url":"https://a.com"},{"url":"https://b.com"}]"#;
    let resp = app
        .clone()
        .oneshot(
            Request::post("/admin/import")
                .header("content-type", "application/json")
                .header("x-admin-token", "secret")
                .body(Body::from(body))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .unwrap();
    let v: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(v["imported"], 2);
    assert_eq!(v["failed"].as_array().unwrap().len(), 0);

    let links_resp = app
        .oneshot(
            Request::get("/admin/links?limit=10")
                .header("x-admin-token", "secret")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let bytes = axum::body::to_bytes(links_resp.into_body(), usize::MAX)
        .await
        .unwrap();
    let v: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    let links = v["links"].as_array().unwrap();
    assert_eq!(links.len(), 2);
}

#[tokio::test]
async fn import_csv_yourls_style_header_maps_correctly() {
    let app = app_admin("secret").await;
    let body = "keyword,url\npromo,https://yourls.example.com\n";
    let resp = app
        .clone()
        .oneshot(
            Request::post("/admin/import")
                .header("content-type", "text/csv")
                .header("x-admin-token", "secret")
                .body(Body::from(body))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .unwrap();
    let v: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(v["imported"], 1);

    let resp = app
        .oneshot(Request::get("/promo").body(Body::empty()).unwrap())
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::FOUND);
    assert_eq!(resp.headers()["location"], "https://yourls.example.com");
}

#[tokio::test]
async fn bad_url_row_reported_while_good_rows_still_import() {
    let app = app_admin("secret").await;
    let body = r#"[{"url":"https://good.com"},{"url":"not-a-url"}]"#;
    let resp = app
        .oneshot(
            Request::post("/admin/import")
                .header("content-type", "application/json")
                .header("x-admin-token", "secret")
                .body(Body::from(body))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .unwrap();
    let v: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(v["imported"], 1);
    let failed = v["failed"].as_array().unwrap();
    assert_eq!(failed.len(), 1);
    assert_eq!(failed[0]["index"], 1);
    assert_eq!(failed[0]["url"], "not-a-url");
    assert_eq!(failed[0]["reason"], "invalid url");
}

#[tokio::test]
async fn alias_collision_reported_in_failed() {
    let app = app_admin("secret").await;
    let body = r#"[{"url":"https://a.com","alias":"dup"},{"url":"https://b.com","alias":"dup"}]"#;
    let resp = app
        .oneshot(
            Request::post("/admin/import")
                .header("content-type", "application/json")
                .header("x-admin-token", "secret")
                .body(Body::from(body))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .unwrap();
    let v: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(v["imported"], 1);
    let failed = v["failed"].as_array().unwrap();
    assert_eq!(failed.len(), 1);
    assert_eq!(failed[0]["index"], 1);
    assert_eq!(failed[0]["reason"], "alias in use");
}

#[tokio::test]
async fn over_cap_returns_400() {
    let app = app_admin("secret").await;
    let rows: Vec<serde_json::Value> = (0..10_001)
        .map(|i| serde_json::json!({ "url": format!("https://x.example.com/{i}") }))
        .collect();
    let body = serde_json::to_vec(&rows).unwrap();
    let resp = app
        .oneshot(
            Request::post("/admin/import")
                .header("content-type", "application/json")
                .header("x-admin-token", "secret")
                .body(Body::from(body))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn without_token_configured_404() {
    let app = app_no_admin().await;
    let resp = app
        .oneshot(
            Request::post("/admin/import")
                .header("content-type", "application/json")
                .body(Body::from(r#"[{"url":"https://a.com"}]"#))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn wrong_token_401() {
    let app = app_admin("secret").await;
    let resp = app
        .oneshot(
            Request::post("/admin/import")
                .header("content-type", "application/json")
                .header("x-admin-token", "wrong")
                .body(Body::from(r#"[{"url":"https://a.com"}]"#))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
}
