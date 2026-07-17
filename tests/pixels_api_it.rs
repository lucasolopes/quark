use axum::body::Body;
use axum::http::{Request, StatusCode};
use quark::api::{router, AppState};
use quark::cache::Cache;
use quark::store::open_backends;
use std::sync::Arc;
use tower::ServiceExt;

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

async fn app_with_token(admin_token: Option<&str>) -> axum::Router {
    let dir = Box::leak(Box::new(tempfile::tempdir().unwrap()));
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
        cache,
        store,
        key: 0x1234,
        signing_key: [0u8; 32],
        analytics_tx,
        sink,
        admin_token: admin_token.map(|s| s.to_string()),
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

async fn app_with_admin(token: &str) -> axum::Router {
    app_with_token(Some(token)).await
}

async fn body_json(resp: axum::response::Response) -> serde_json::Value {
    let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .unwrap();
    serde_json::from_slice(&bytes).unwrap()
}

#[tokio::test]
async fn post_pixels_without_token_header_is_unauthorized() {
    let app = app_with_admin("secret").await;
    let resp = app
        .oneshot(
            Request::post("/admin/pixels")
                .header("content-type", "application/json")
                .body(Body::from(
                    r#"{"provider":"ga4","credentials":{"measurement_id":"G-1","api_secret":"s"}}"#,
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn post_pixels_endpoint_disabled_when_admin_not_configured() {
    let app = app_with_token(None).await;
    let resp = app
        .oneshot(
            Request::post("/admin/pixels")
                .header("content-type", "application/json")
                .body(Body::from(
                    r#"{"provider":"ga4","credentials":{"measurement_id":"G-1","api_secret":"s"}}"#,
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn post_pixels_missing_required_credentials_400() {
    let app = app_with_admin("secret").await;
    let resp = app
        .oneshot(
            Request::post("/admin/pixels")
                .header("content-type", "application/json")
                .header("x-admin-token", "secret")
                .body(Body::from(
                    r#"{"provider":"ga4","credentials":{"measurement_id":"G-1"}}"#,
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn post_meta_missing_required_credentials_400() {
    let app = app_with_admin("secret").await;
    let resp = app
        .oneshot(
            Request::post("/admin/pixels")
                .header("content-type", "application/json")
                .header("x-admin-token", "secret")
                .body(Body::from(
                    r#"{"provider":"meta_capi","credentials":{"pixel_id":"123"}}"#,
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn post_pixels_creates_and_get_masks_credentials() {
    let app = app_with_admin("secret").await;
    let resp = app
        .clone()
        .oneshot(
            Request::post("/admin/pixels")
                .header("content-type", "application/json")
                .header("x-admin-token", "secret")
                .body(Body::from(
                    r#"{"provider":"ga4","credentials":{"measurement_id":"G-1","api_secret":"topsecret"}}"#,
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::CREATED);
    let created = body_json(resp).await;
    assert_eq!(
        created["credentials"]["api_secret"],
        "\u{2022}\u{2022}\u{2022}\u{2022}"
    );
    assert_ne!(created["credentials"]["api_secret"], "topsecret");
    let id = created["id"].as_u64().unwrap();

    let resp = app
        .clone()
        .oneshot(
            Request::get("/admin/pixels")
                .header("x-admin-token", "secret")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let listed = body_json(resp).await;
    let pixels = listed["pixels"].as_array().unwrap();
    assert_eq!(pixels.len(), 1);
    assert_eq!(pixels[0]["id"], id);
    assert_eq!(
        pixels[0]["credentials"]["api_secret"],
        "\u{2022}\u{2022}\u{2022}\u{2022}"
    );
    assert_eq!(pixels[0]["credentials"]["measurement_id"], "G-1");
    assert_eq!(pixels[0]["active"], true);

    let resp = app
        .clone()
        .oneshot(
            Request::delete(format!("/admin/pixels/{id}"))
                .header("x-admin-token", "secret")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::NO_CONTENT);

    let resp = app
        .clone()
        .oneshot(
            Request::delete(format!("/admin/pixels/{id}"))
                .header("x-admin-token", "secret")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);

    let resp = app
        .oneshot(
            Request::get("/admin/pixels")
                .header("x-admin-token", "secret")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let listed = body_json(resp).await;
    assert!(listed["pixels"].as_array().unwrap().is_empty());
}

#[tokio::test]
async fn get_pixels_endpoint_disabled_when_admin_not_configured() {
    let app = app_with_token(None).await;
    let resp = app
        .oneshot(Request::get("/admin/pixels").body(Body::empty()).unwrap())
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
}
