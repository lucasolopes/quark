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

async fn app_admin(token: &str) -> axum::Router {
    let dir = Box::leak(Box::new(tempfile::tempdir().unwrap()));
    let (store, sink) = open_backends(dir.path()).await.unwrap();
    let cache = Cache::new(store.clone(), 1000, None);
    let (tx, _rx) = tokio::sync::mpsc::channel(100);
    let state = Arc::new(AppState {
        oidc: None,
        oidc_configured: false,
        cache,
        store,
        key: 0x1234,
        signing_key: [0u8; 32],
        analytics_tx: tx,
        sink,
        admin_token: Some(token.to_string()),
        ratelimiter: quark::abuse::ratelimit::RateLimiter::memory(1000),
        block_private: true,
        public_host: None,
        real_ip_header: "cf-connecting-ip".to_string(),
        webhooks: test_webhook_dispatcher(),
    });
    router(state)
}

async fn create_token(
    app: &axum::Router,
    admin_token: &str,
    body: &str,
) -> (StatusCode, serde_json::Value) {
    let resp = app
        .clone()
        .oneshot(
            Request::post("/admin/tokens")
                .header("content-type", "application/json")
                .header("x-admin-token", admin_token)
                .body(Body::from(body.to_string()))
                .unwrap(),
        )
        .await
        .unwrap();
    let status = resp.status();
    let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .unwrap();
    let v: serde_json::Value = serde_json::from_slice(&bytes).unwrap_or(serde_json::Value::Null);
    (status, v)
}

#[tokio::test]
async fn post_admin_tokens_with_env_admin_token_returns_plaintext_once() {
    let app = app_admin("secret").await;
    let (status, v) =
        create_token(&app, "secret", r#"{"name":"ci","scopes":["links_read"]}"#).await;
    assert_eq!(status, StatusCode::CREATED);
    assert!(v["id"].is_u64());
    let token = v["token"].as_str().unwrap();
    assert!(token.starts_with("qtok_"));
}

#[tokio::test]
async fn links_read_scope_can_list_but_not_delete_links() {
    let app = app_admin("secret").await;
    let (_, v) = create_token(
        &app,
        "secret",
        r#"{"name":"reader","scopes":["links_read"]}"#,
    )
    .await;
    let tok = v["token"].as_str().unwrap();

    let resp = app
        .clone()
        .oneshot(
            Request::get("/admin/links")
                .header("x-admin-token", tok)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    let resp = app
        .oneshot(
            Request::delete("/admin/links/0000000")
                .header("x-admin-token", tok)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn revoked_token_becomes_401_when_env_token_configured() {
    let app = app_admin("secret").await;
    let (_, v) = create_token(
        &app,
        "secret",
        r#"{"name":"disposable","scopes":["links_read"]}"#,
    )
    .await;
    let id = v["id"].as_u64().unwrap();
    let tok = v["token"].as_str().unwrap().to_string();

    let resp = app
        .clone()
        .oneshot(
            Request::get("/admin/links")
                .header("x-admin-token", &tok)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    let resp = app
        .clone()
        .oneshot(
            Request::delete(format!("/admin/tokens/{id}"))
                .header("x-admin-token", "secret")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::NO_CONTENT);

    let resp = app
        .oneshot(
            Request::get("/admin/links")
                .header("x-admin-token", &tok)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn delete_nonexistent_token_404() {
    let app = app_admin("secret").await;
    let resp = app
        .oneshot(
            Request::delete("/admin/tokens/999999")
                .header("x-admin-token", "secret")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn low_rate_limit_per_min_returns_429_after_exceeding() {
    let app = app_admin("secret").await;
    let (_, v) = create_token(
        &app,
        "secret",
        r#"{"name":"throttled","scopes":["links_read"],"rate_limit_per_min":1}"#,
    )
    .await;
    let tok = v["token"].as_str().unwrap();

    let resp = app
        .clone()
        .oneshot(
            Request::get("/admin/links")
                .header("x-admin-token", tok)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    let resp = app
        .oneshot(
            Request::get("/admin/links")
                .header("x-admin-token", tok)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::TOO_MANY_REQUESTS);
}

#[tokio::test]
async fn get_admin_tokens_never_leaks_hash_or_plaintext() {
    let app = app_admin("secret").await;
    let (_, v) = create_token(&app, "secret", r#"{"name":"ci","scopes":["links_read"]}"#).await;
    let plaintext = v["token"].as_str().unwrap().to_string();

    let resp = app
        .oneshot(
            Request::get("/admin/tokens")
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
    let body_str = String::from_utf8(bytes.to_vec()).unwrap();
    assert!(!body_str.contains(&plaintext));
    assert!(!body_str.contains("token_hash"));
    let v: serde_json::Value = serde_json::from_str(&body_str).unwrap();
    let tokens = v["tokens"].as_array().unwrap();
    assert_eq!(tokens.len(), 1);
    assert_eq!(tokens[0]["name"], "ci");
    assert!(tokens[0].get("token").is_none());
    assert!(tokens[0].get("token_hash").is_none());
}

#[tokio::test]
async fn full_scope_token_can_manage_tokens() {
    let app = app_admin("secret").await;
    let (_, v) = create_token(&app, "secret", r#"{"name":"super","scopes":["full"]}"#).await;
    let tok = v["token"].as_str().unwrap().to_string();

    let (status, _) = create_token(&app, &tok, r#"{"name":"child","scopes":["links_read"]}"#).await;
    assert_eq!(status, StatusCode::CREATED);
}

#[tokio::test]
async fn api_token_works_when_no_env_admin_token_is_configured() {
    let dir = Box::leak(Box::new(tempfile::tempdir().unwrap()));
    let (store, sink) = open_backends(dir.path()).await.unwrap();
    let cache = Cache::new(store.clone(), 1000, None);
    let (tx, _rx) = tokio::sync::mpsc::channel(100);

    let id = store.next_api_token_id().await.unwrap();
    let plaintext = quark::auth::generate_token();
    let token = quark::auth::ApiToken {
        id,
        name: "standalone".into(),
        token_hash: quark::auth::hash_token(&plaintext),
        scopes: vec![quark::auth::Scope::LinksRead],
        rate_limit_per_min: None,
        created: 1,
    };
    store.put_api_token(&token).await.unwrap();
    let state = Arc::new(AppState {
        oidc: None,
        oidc_configured: false,
        cache,
        store,
        key: 0x1234,
        signing_key: [0u8; 32],
        analytics_tx: tx,
        sink,
        admin_token: None,
        ratelimiter: quark::abuse::ratelimit::RateLimiter::memory(1000),
        block_private: true,
        public_host: None,
        real_ip_header: "cf-connecting-ip".to_string(),
        webhooks: test_webhook_dispatcher(),
    });
    let app = router(state);

    let resp = app
        .oneshot(
            Request::get("/admin/links")
                .header("x-admin-token", &plaintext)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
}

async fn post_root_create(app: &axum::Router, token: &str) -> StatusCode {
    app.clone()
        .oneshot(
            Request::post("/")
                .header("content-type", "application/json")
                .header("x-admin-token", token)
                .body(Body::from(r#"{"url":"https://example.com"}"#))
                .unwrap(),
        )
        .await
        .unwrap()
        .status()
}

#[tokio::test]
async fn links_write_scope_can_create_via_post_root() {
    let app = app_admin("secret").await;
    let (status, v) = create_token(
        &app,
        "secret",
        r#"{"name":"writer","scopes":["links_write"]}"#,
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    let token = v["token"].as_str().unwrap();
    assert_eq!(post_root_create(&app, token).await, StatusCode::OK);
}

#[tokio::test]
async fn links_read_scope_cannot_create_via_post_root() {
    let app = app_admin("secret").await;
    let (status, v) = create_token(
        &app,
        "secret",
        r#"{"name":"reader","scopes":["links_read"]}"#,
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    let token = v["token"].as_str().unwrap();
    assert_eq!(post_root_create(&app, token).await, StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn env_admin_token_still_creates_via_post_root() {
    let app = app_admin("secret").await;
    assert_eq!(post_root_create(&app, "secret").await, StatusCode::OK);
}

#[tokio::test]
async fn unknown_token_cannot_create_via_post_root() {
    let app = app_admin("secret").await;
    assert_eq!(
        post_root_create(&app, "qtok_bogus").await,
        StatusCode::UNAUTHORIZED
    );
}
