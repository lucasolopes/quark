use axum::body::Body;
use axum::http::{Request, StatusCode};
use quark::analytics::ClickEvent;
use quark::api::{router, AppState};
use quark::cache::Cache;
use quark::store::open_backends;
use std::sync::Arc;
use tower::ServiceExt;

async fn app() -> axum::Router {
    let dir = Box::leak(Box::new(tempfile::tempdir().unwrap()));
    let (store, sink) = open_backends(dir.path()).await.unwrap();
    let cache = Cache::new(store.clone(), 1000);
    let (analytics_tx, _rx) = tokio::sync::mpsc::channel(100);
    let store2 = store.clone();
    let state = Arc::new(AppState {
        cache,
        store,
        key: 0x1234,
        analytics_tx,
        sink,
        admin_token: None,
        ratelimiter: quark::abuse::ratelimit::RateLimiter::disabled(),
        blocklist: quark::abuse::blocklist::Blocklist::new(store2, None, 60),
        block_private: true,
        public_host: None,
        real_ip_header: "cf-connecting-ip".to_string(),
        webhooks: test_webhook_dispatcher(),
    });
    router(state)
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

#[tokio::test]
async fn creates_and_redirects() {
    let app = app().await;
    let resp = app
        .clone()
        .oneshot(
            Request::post("/")
                .header("content-type", "application/json")
                .body(Body::from(r#"{"url":"https://example.com"}"#))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .unwrap();
    let v: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    let code = v["code"].as_str().unwrap().to_string();

    let resp = app
        .oneshot(
            Request::get(format!("/{code}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::FOUND);
    assert_eq!(resp.headers()["location"], "https://example.com");
}

#[tokio::test]
async fn nonexistent_code_404() {
    let app = app().await;
    let resp = app
        .oneshot(Request::get("/0000000").body(Body::empty()).unwrap())
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn alias_in_use_409() {
    let app = app().await;
    let resp = app
        .clone()
        .oneshot(
            Request::post("/")
                .header("content-type", "application/json")
                .body(Body::from(r#"{"url":"https://a.com","alias":"promo"}"#))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    let resp = app
        .oneshot(
            Request::post("/")
                .header("content-type", "application/json")
                .body(Body::from(r#"{"url":"https://b.com","alias":"promo"}"#))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::CONFLICT);
}

#[tokio::test]
async fn expired_link_410() {
    let app = app().await;
    let resp = app
        .clone()
        .oneshot(
            Request::post("/")
                .header("content-type", "application/json")
                .body(Body::from(r#"{"url":"https://a.com","ttl":0}"#))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .unwrap();
    let v: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    let code = v["code"].as_str().unwrap().to_string();

    let resp = app
        .oneshot(
            Request::get(format!("/{code}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::GONE);
}

#[tokio::test]
async fn alias_redirects() {
    let app = app().await;
    let resp = app
        .clone()
        .oneshot(
            Request::post("/")
                .header("content-type", "application/json")
                .body(Body::from(
                    r#"{"url":"https://alias.example.com","alias":"promo"}"#,
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    let resp = app
        .oneshot(Request::get("/promo").body(Body::empty()).unwrap())
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::FOUND);
    assert_eq!(resp.headers()["location"], "https://alias.example.com");
}

#[tokio::test]
async fn numeric_alias_rejected() {
    let app = app().await;
    let resp = app
        .oneshot(
            Request::post("/")
                .header("content-type", "application/json")
                .body(Body::from(r#"{"url":"https://x.com","alias":"0000000"}"#))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn blocks_internal_destination_403() {
    let app = app().await;
    let resp = app
        .oneshot(
            Request::post("/")
                .header("content-type", "application/json")
                .body(Body::from(r#"{"url":"http://127.0.0.1:8080/x"}"#))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn blocks_domain_on_blocklist_403() {
    let dir = Box::leak(Box::new(tempfile::tempdir().unwrap()));
    let (store, sink) = open_backends(dir.path()).await.unwrap();
    store.add_blocked_domain("evil.com").await.unwrap();
    let cache = Cache::new(store.clone(), 1000);
    let (tx, _rx) = tokio::sync::mpsc::channel(100);
    let state = Arc::new(AppState {
        cache,
        store: store.clone(),
        key: 0x1234,
        analytics_tx: tx,
        sink,
        admin_token: None,
        ratelimiter: quark::abuse::ratelimit::RateLimiter::disabled(),
        blocklist: quark::abuse::blocklist::Blocklist::new(store, None, 60),
        block_private: true,
        public_host: None,
        real_ip_header: "cf-connecting-ip".to_string(),
        webhooks: test_webhook_dispatcher(),
    });
    let app = router(state);
    let resp = app
        .oneshot(
            Request::post("/")
                .header("content-type", "application/json")
                .body(Body::from(r#"{"url":"https://sub.evil.com/x"}"#))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn rate_limit_429_after_exceeding() {
    let dir = Box::leak(Box::new(tempfile::tempdir().unwrap()));
    let (store, sink) = open_backends(dir.path()).await.unwrap();
    let cache = Cache::new(store.clone(), 1000);
    let (tx, _rx) = tokio::sync::mpsc::channel(100);
    let store2 = store.clone();
    let state = Arc::new(AppState {
        cache,
        store,
        key: 0x1234,
        analytics_tx: tx,
        sink,
        admin_token: None,
        ratelimiter: quark::abuse::ratelimit::RateLimiter::memory(1),
        blocklist: quark::abuse::blocklist::Blocklist::new(store2, None, 60),
        block_private: true,
        public_host: None,
        real_ip_header: "cf-connecting-ip".to_string(),
        webhooks: test_webhook_dispatcher(),
    });
    let app = router(state);
    let mk = || {
        Request::post("/")
            .header("content-type", "application/json")
            .header("cf-connecting-ip", "9.9.9.9")
            .body(Body::from(r#"{"url":"https://ok.com/x"}"#))
            .unwrap()
    };
    assert_eq!(
        app.clone().oneshot(mk()).await.unwrap().status(),
        StatusCode::OK
    );
    assert_eq!(
        app.oneshot(mk()).await.unwrap().status(),
        StatusCode::TOO_MANY_REQUESTS
    );
}

#[tokio::test]
async fn redirect_without_ttl_has_default_cache_control() {
    let app = app().await;
    let resp = app
        .clone()
        .oneshot(
            Request::post("/")
                .header("content-type", "application/json")
                .body(Body::from(r#"{"url":"https://example.com"}"#))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .unwrap();
    let v: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    let code = v["code"].as_str().unwrap().to_string();

    let resp = app
        .oneshot(
            Request::get(format!("/{code}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::FOUND);
    assert_eq!(resp.headers()["cache-control"], "public, max-age=86400");
}

#[tokio::test]
async fn redirect_with_ttl_has_cache_control_capped_by_ttl() {
    let app = app().await;
    let resp = app
        .clone()
        .oneshot(
            Request::post("/")
                .header("content-type", "application/json")
                .body(Body::from(r#"{"url":"https://example.com","ttl":100}"#))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .unwrap();
    let v: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    let code = v["code"].as_str().unwrap().to_string();

    let resp = app
        .oneshot(
            Request::get(format!("/{code}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::FOUND);
    let cc = resp.headers()["cache-control"]
        .to_str()
        .unwrap()
        .to_string();
    let max_age: i64 = cc
        .strip_prefix("public, max-age=")
        .expect("should be public, max-age=<n>")
        .parse()
        .expect("max-age should be numeric");
    assert!(
        max_age > 0 && max_age <= 100,
        "max-age outside expected range: {max_age}"
    );
}

#[tokio::test]
async fn nonexistent_code_404_has_no_store_cache_control() {
    let app = app().await;
    let resp = app
        .oneshot(Request::get("/0000000").body(Body::empty()).unwrap())
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
    assert_eq!(resp.headers()["cache-control"], "no-store");
}

async fn app_admin(token: &str) -> axum::Router {
    let dir = Box::leak(Box::new(tempfile::tempdir().unwrap()));
    let (store, sink) = open_backends(dir.path()).await.unwrap();
    let cache = Cache::new(store.clone(), 1000);
    let (tx, _rx) = tokio::sync::mpsc::channel(100);
    let store2 = store.clone();
    let state = Arc::new(AppState {
        cache,
        store,
        key: 0x1234,
        analytics_tx: tx,
        sink,
        admin_token: Some(token.to_string()),
        ratelimiter: quark::abuse::ratelimit::RateLimiter::disabled(),
        blocklist: quark::abuse::blocklist::Blocklist::new(store2, None, 60),
        block_private: true,
        public_host: None,
        real_ip_header: "cf-connecting-ip".to_string(),
        webhooks: test_webhook_dispatcher(),
    });
    router(state)
}

#[tokio::test]
async fn admin_blocklist_add_list_and_blocks() {
    let app = app_admin("secret").await;
    let resp = app
        .clone()
        .oneshot(
            Request::post("/admin/blocklist")
                .header("content-type", "application/json")
                .header("x-admin-token", "secret")
                .body(Body::from(r#"{"domain":"evil.com"}"#))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let resp = app
        .clone()
        .oneshot(
            Request::get("/admin/blocklist")
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
    assert_eq!(v["domains"][0], "evil.com");
}

#[tokio::test]
async fn admin_blocklist_without_token_404() {
    let app = app().await;
    let resp = app
        .oneshot(
            Request::get("/admin/blocklist")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn admin_blocklist_token_wrong_401() {
    let app = app_admin("secret").await;
    let resp = app
        .oneshot(
            Request::get("/admin/blocklist")
                .header("x-admin-token", "wrong")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn ttl_overflow_400() {
    let app = app().await;
    let resp = app
        .oneshot(
            Request::post("/")
                .header("content-type", "application/json")
                .body(Body::from(
                    r#"{"url":"https://a.com","ttl":18446744073709551615}"#,
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn admin_blocklist_without_token_malformed_body_404() {
    let app = app().await;
    let resp = app
        .oneshot(
            Request::post("/admin/blocklist")
                .header("content-type", "application/json")
                .body(Body::from("not json"))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn admin_blocklist_delete_remove() {
    let app = app_admin("secret").await;
    app.clone()
        .oneshot(
            Request::post("/admin/blocklist")
                .header("content-type", "application/json")
                .header("x-admin-token", "secret")
                .body(Body::from(r#"{"domain":"del.com"}"#))
                .unwrap(),
        )
        .await
        .unwrap();
    let resp = app
        .clone()
        .oneshot(
            Request::delete("/admin/blocklist")
                .header("content-type", "application/json")
                .header("x-admin-token", "secret")
                .body(Body::from(r#"{"domain":"del.com"}"#))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let resp = app
        .oneshot(
            Request::get("/admin/blocklist")
                .header("x-admin-token", "secret")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .unwrap();
    let v: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(v["domains"].as_array().unwrap().len(), 0);
}

#[tokio::test]
async fn admin_links_paginated_list() {
    let app = app_admin("secret").await;
    for u in ["https://a.com", "https://b.com"] {
        app.clone()
            .oneshot(
                Request::post("/")
                    .header("content-type", "application/json")
                    .header("x-admin-token", "secret")
                    .body(Body::from(format!(r#"{{"url":"{u}"}}"#)))
                    .unwrap(),
            )
            .await
            .unwrap();
    }
    let resp = app
        .oneshot(
            Request::get("/admin/links?limit=10")
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
    let links = v["links"].as_array().unwrap();
    assert_eq!(links.len(), 2);
    assert!(links[0]["code"].as_str().unwrap().len() == 7);
    assert_eq!(links[0]["url"], "https://a.com");
    assert!(
        v["next_after"].is_null(),
        "next_after should be null on a partial page"
    );
}

#[tokio::test]
async fn create_with_tags_then_filter_by_tag() {
    let app = app_admin("secret").await;
    for (u, tags) in [
        ("https://a.com", r#"["Rust", "Web"]"#),
        ("https://b.com", r#"["web"]"#),
        ("https://c.com", "[]"),
    ] {
        let resp = app
            .clone()
            .oneshot(
                Request::post("/")
                    .header("content-type", "application/json")
                    .header("x-admin-token", "secret")
                    .body(Body::from(format!(r#"{{"url":"{u}","tags":{tags}}}"#)))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
    }

    let resp = app
        .clone()
        .oneshot(
            Request::get("/admin/links?tag=rust")
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
    let links = v["links"].as_array().unwrap();
    assert_eq!(links.len(), 1, "only the link tagged 'rust' matches");
    assert_eq!(links[0]["url"], "https://a.com");
    assert_eq!(links[0]["tags"], serde_json::json!(["rust", "web"]));

    let resp = app
        .clone()
        .oneshot(
            Request::get("/admin/links?tag=RUST")
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
    let links = v["links"].as_array().unwrap();
    assert_eq!(
        links.len(),
        1,
        "uppercase filter must match the lowercase stored tag"
    );
    assert_eq!(links[0]["url"], "https://a.com");

    let resp = app
        .oneshot(
            Request::get("/admin/links?tag=web")
                .header("x-admin-token", "secret")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .unwrap();
    let v: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(
        v["links"].as_array().unwrap().len(),
        2,
        "a and b both have 'web'"
    );
}

#[tokio::test]
async fn admin_tags_returns_distinct_set() {
    let app = app_admin("secret").await;
    for (u, tags) in [
        ("https://a.com", r#"["rust", "web"]"#),
        ("https://b.com", r#"["web", "cli"]"#),
    ] {
        app.clone()
            .oneshot(
                Request::post("/")
                    .header("content-type", "application/json")
                    .header("x-admin-token", "secret")
                    .body(Body::from(format!(r#"{{"url":"{u}","tags":{tags}}}"#)))
                    .unwrap(),
            )
            .await
            .unwrap();
    }
    let resp = app
        .oneshot(
            Request::get("/admin/tags")
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
    let mut tags: Vec<String> = v["tags"]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| t.as_str().unwrap().to_string())
        .collect();
    tags.sort();
    assert_eq!(
        tags,
        vec!["cli".to_string(), "rust".to_string(), "web".to_string()]
    );
}

#[tokio::test]
async fn admin_links_search_on_lmdb_returns_501() {
    let app = app_admin("secret").await;
    let resp = app
        .oneshot(
            Request::get("/admin/links?q=abc")
                .header("x-admin-token", "secret")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::NOT_IMPLEMENTED);
}

#[tokio::test]
async fn admin_links_without_token_404() {
    let app = app().await;
    let resp = app
        .oneshot(Request::get("/admin/links").body(Body::empty()).unwrap())
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
}

async fn create_and_get_code(app: &axum::Router, url: &str) -> String {
    let resp = app
        .clone()
        .oneshot(
            Request::post("/")
                .header("content-type", "application/json")
                .header("x-admin-token", "secret")
                .body(Body::from(format!(r#"{{"url":"{url}"}}"#)))
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
async fn admin_delete_link_becomes_404_on_redirect() {
    let app = app_admin("secret").await;
    let code = create_and_get_code(&app, "https://del.com").await;
    let r = app
        .clone()
        .oneshot(
            Request::get(format!("/{code}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(r.status(), StatusCode::FOUND);
    let r = app
        .clone()
        .oneshot(
            Request::delete(format!("/admin/links/{code}"))
                .header("x-admin-token", "secret")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(r.status(), StatusCode::OK);
    let r = app
        .oneshot(
            Request::get(format!("/{code}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(r.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn admin_patch_link_updates_destination() {
    let app = app_admin("secret").await;
    let code = create_and_get_code(&app, "https://old.com").await;
    let r = app
        .clone()
        .oneshot(
            Request::patch(format!("/admin/links/{code}"))
                .header("x-admin-token", "secret")
                .header("content-type", "application/json")
                .body(Body::from(r#"{"url":"https://new.com"}"#))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(r.status(), StatusCode::OK);
    let r = app
        .oneshot(
            Request::get(format!("/{code}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(r.status(), StatusCode::FOUND);
    assert_eq!(r.headers()["location"], "https://new.com");
}

#[tokio::test]
async fn admin_patch_link_replaces_tags() {
    let app = app_admin("secret").await;
    let code = create_and_get_code(&app, "https://tagged.com").await;
    let r = app
        .clone()
        .oneshot(
            Request::patch(format!("/admin/links/{code}"))
                .header("x-admin-token", "secret")
                .header("content-type", "application/json")
                .body(Body::from(r#"{"tags":["Rust", "rust", " web "]}"#))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(r.status(), StatusCode::OK);
    let r = app
        .oneshot(
            Request::get("/admin/links?limit=10")
                .header("x-admin-token", "secret")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let bytes = axum::body::to_bytes(r.into_body(), usize::MAX)
        .await
        .unwrap();
    let v: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    let links = v["links"].as_array().unwrap();
    assert_eq!(links[0]["tags"], serde_json::json!(["rust", "web"]));
}

#[tokio::test]
async fn admin_delete_nonexistent_404() {
    let app = app_admin("secret").await;
    let r = app
        .oneshot(
            Request::delete("/admin/links/0000000")
                .header("x-admin-token", "secret")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(r.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn admin_patch_internal_destination_403() {
    let app = app_admin("secret").await;
    let code = create_and_get_code(&app, "https://ok.com").await;
    let r = app
        .oneshot(
            Request::patch(format!("/admin/links/{code}"))
                .header("x-admin-token", "secret")
                .header("content-type", "application/json")
                .body(Body::from(r#"{"url":"http://127.0.0.1:9000"}"#))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(r.status(), StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn admin_patch_invalid_url_400() {
    let app = app_admin("secret").await;
    let code = create_and_get_code(&app, "https://ok.com").await;
    let r = app
        .oneshot(
            Request::patch(format!("/admin/links/{code}"))
                .header("x-admin-token", "secret")
                .header("content-type", "application/json")
                .body(Body::from(r#"{"url":"ftp://nope"}"#))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(r.status(), StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn create_without_token_when_configured_401() {
    let app = app_admin("secret").await;
    let resp = app
        .oneshot(
            Request::post("/")
                .header("content-type", "application/json")
                .body(Body::from(r#"{"url":"https://example.com"}"#))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn create_with_token_when_configured_ok() {
    let app = app_admin("secret").await;
    let resp = app
        .oneshot(
            Request::post("/")
                .header("content-type", "application/json")
                .header("x-admin-token", "secret")
                .body(Body::from(r#"{"url":"https://example.com"}"#))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
}

/// Builds an app like `app()` but returns the analytics receiver too, so
/// tests can inspect the `ClickEvent` (in particular `variant`) that the
/// redirect handler sends.
async fn app_with_analytics_rx() -> (axum::Router, tokio::sync::mpsc::Receiver<ClickEvent>) {
    let dir = Box::leak(Box::new(tempfile::tempdir().unwrap()));
    let (store, sink) = open_backends(dir.path()).await.unwrap();
    let cache = Cache::new(store.clone(), 1000);
    let (analytics_tx, rx) = tokio::sync::mpsc::channel(100);
    let store2 = store.clone();
    let state = Arc::new(AppState {
        cache,
        store,
        key: 0x1234,
        analytics_tx,
        sink,
        admin_token: None,
        ratelimiter: quark::abuse::ratelimit::RateLimiter::disabled(),
        blocklist: quark::abuse::blocklist::Blocklist::new(store2, None, 60),
        block_private: true,
        public_host: None,
        real_ip_header: "cf-connecting-ip".to_string(),
        webhooks: test_webhook_dispatcher(),
    });
    (router(state), rx)
}

#[tokio::test]
async fn redirect_with_two_variants_picks_one_of_the_urls_and_sets_click_event_variant() {
    let (app, mut rx) = app_with_analytics_rx().await;
    let resp = app
        .clone()
        .oneshot(
            Request::post("/")
                .header("content-type", "application/json")
                .body(Body::from(
                    r#"{"url":"https://default.com","variants":[
                        {"url":"https://variant-a.com","weight":1},
                        {"url":"https://variant-b.com","weight":1}
                    ]}"#,
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .unwrap();
    let v: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    let code = v["code"].as_str().unwrap().to_string();

    let resp = app
        .oneshot(
            Request::get(format!("/{code}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::FOUND);
    let location = resp.headers()["location"].to_str().unwrap().to_string();
    assert!(
        location == "https://variant-a.com" || location == "https://variant-b.com",
        "unexpected location: {location}"
    );

    let ev = rx.try_recv().expect("redirect should send a ClickEvent");
    assert!(
        ev.variant == Some(0) || ev.variant == Some(1),
        "expected Some(0|1), got {:?}",
        ev.variant
    );
}

#[tokio::test]
async fn redirect_without_variants_uses_url_and_variant_is_none() {
    let (app, mut rx) = app_with_analytics_rx().await;
    let resp = app
        .clone()
        .oneshot(
            Request::post("/")
                .header("content-type", "application/json")
                .body(Body::from(r#"{"url":"https://plain.com"}"#))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .unwrap();
    let v: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    let code = v["code"].as_str().unwrap().to_string();

    let resp = app
        .oneshot(
            Request::get(format!("/{code}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::FOUND);
    assert_eq!(resp.headers()["location"], "https://plain.com");

    let ev = rx.try_recv().expect("redirect should send a ClickEvent");
    assert_eq!(ev.variant, None);
}

#[tokio::test]
async fn create_with_internal_variant_url_is_rejected() {
    let app = app().await;
    let resp = app
        .oneshot(
            Request::post("/")
                .header("content-type", "application/json")
                .body(Body::from(
                    r#"{"url":"https://ok.com","variants":[
                        {"url":"http://127.0.0.1:8080","weight":1}
                    ]}"#,
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn create_with_too_many_variants_400() {
    let app = app().await;
    let variants: Vec<String> = (0..11)
        .map(|i| format!(r#"{{"url":"https://v{i}.com","weight":1}}"#))
        .collect();
    let body = format!(
        r#"{{"url":"https://ok.com","variants":[{}]}}"#,
        variants.join(",")
    );
    let resp = app
        .oneshot(
            Request::post("/")
                .header("content-type", "application/json")
                .body(Body::from(body))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn create_with_zero_weight_variant_400() {
    let app = app().await;
    let resp = app
        .oneshot(
            Request::post("/")
                .header("content-type", "application/json")
                .body(Body::from(
                    r#"{"url":"https://ok.com","variants":[
                        {"url":"https://a.com","weight":0}
                    ]}"#,
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn patch_with_internal_variant_url_is_rejected() {
    let app = app_admin("secret").await;
    let code = create_and_get_code(&app, "https://ok.com").await;
    let r = app
        .oneshot(
            Request::patch(format!("/admin/links/{code}"))
                .header("x-admin-token", "secret")
                .header("content-type", "application/json")
                .body(Body::from(
                    r#"{"variants":[{"url":"http://127.0.0.1:9000","weight":1}]}"#,
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(r.status(), StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn patch_variants_round_trips_through_admin_list() {
    let app = app_admin("secret").await;
    let code = create_and_get_code(&app, "https://ok.com").await;
    let r = app
        .clone()
        .oneshot(
            Request::patch(format!("/admin/links/{code}"))
                .header("x-admin-token", "secret")
                .header("content-type", "application/json")
                .body(Body::from(
                    r#"{"variants":[{"url":"https://x.com","weight":2}]}"#,
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(r.status(), StatusCode::OK);

    let r = app
        .oneshot(
            Request::get("/admin/links")
                .header("x-admin-token", "secret")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(r.status(), StatusCode::OK);
    let bytes = axum::body::to_bytes(r.into_body(), usize::MAX)
        .await
        .unwrap();
    let v: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    let links = v["links"].as_array().unwrap();
    let link = links
        .iter()
        .find(|l| l["code"].as_str().unwrap() == code)
        .unwrap();
    assert_eq!(link["variants"][0]["url"], "https://x.com");
    assert_eq!(link["variants"][0]["weight"], 2);
}

#[tokio::test]
async fn max_visits_expires_after_limit() {
    let app = app_admin("secret").await;
    let resp = app
        .clone()
        .oneshot(
            Request::post("/")
                .header("content-type", "application/json")
                .header("x-admin-token", "secret")
                .body(Body::from(
                    r#"{"url":"https://example.com","max_visits":2}"#,
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .unwrap();
    let v: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    let code = v["code"].as_str().unwrap().to_string();

    for _ in 0..2 {
        let r = app
            .clone()
            .oneshot(
                Request::get(format!("/{code}"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(r.status(), StatusCode::FOUND);
    }
    let r = app
        .oneshot(
            Request::get(format!("/{code}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(r.status(), StatusCode::GONE);
}

#[tokio::test]
async fn link_without_max_visits_redirects_unlimited_and_counter_stays_zero() {
    let app = app_admin("secret").await;
    let code = create_and_get_code(&app, "https://example.com").await;
    for _ in 0..5 {
        let r = app
            .clone()
            .oneshot(
                Request::get(format!("/{code}"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(r.status(), StatusCode::FOUND);
    }
    let r = app
        .oneshot(
            Request::get("/admin/links?limit=10")
                .header("x-admin-token", "secret")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let bytes = axum::body::to_bytes(r.into_body(), usize::MAX)
        .await
        .unwrap();
    let v: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    let links = v["links"].as_array().unwrap();
    let row = links
        .iter()
        .find(|l| l["code"] == code)
        .expect("created link should be in the list");
    assert!(row["max_visits"].is_null());
    assert_eq!(
        row["visits"], 0,
        "bump_visits must never be called on the hot path when max_visits is None"
    );
}

#[tokio::test]
async fn admin_patch_sets_and_clears_max_visits() {
    let app = app_admin("secret").await;
    let code = create_and_get_code(&app, "https://example.com").await;

    let r = app
        .clone()
        .oneshot(
            Request::patch(format!("/admin/links/{code}"))
                .header("x-admin-token", "secret")
                .header("content-type", "application/json")
                .body(Body::from(r#"{"max_visits":3}"#))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(r.status(), StatusCode::OK);

    let r = app
        .clone()
        .oneshot(
            Request::get("/admin/links?limit=10")
                .header("x-admin-token", "secret")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let bytes = axum::body::to_bytes(r.into_body(), usize::MAX)
        .await
        .unwrap();
    let v: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    let links = v["links"].as_array().unwrap();
    let row = links.iter().find(|l| l["code"] == code).unwrap();
    assert_eq!(row["max_visits"], 3);

    let r = app
        .clone()
        .oneshot(
            Request::patch(format!("/admin/links/{code}"))
                .header("x-admin-token", "secret")
                .header("content-type", "application/json")
                .body(Body::from(r#"{"max_visits":null}"#))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(r.status(), StatusCode::OK);

    let r = app
        .oneshot(
            Request::get("/admin/links?limit=10")
                .header("x-admin-token", "secret")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let bytes = axum::body::to_bytes(r.into_body(), usize::MAX)
        .await
        .unwrap();
    let v: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    let links = v["links"].as_array().unwrap();
    let row = links.iter().find(|l| l["code"] == code).unwrap();
    assert!(row["max_visits"].is_null());
}

#[tokio::test]
async fn wellknown_put_then_public_get() {
    let app = app_admin("secret").await;
    let body = r#"{"relation":["delegate_permission/common.handle_all_urls"]}"#;
    let resp = app
        .clone()
        .oneshot(
            Request::put("/admin/wellknown/assetlinks.json")
                .header("x-admin-token", "secret")
                .body(Body::from(body))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    let resp = app
        .oneshot(
            Request::get("/.well-known/assetlinks.json")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    assert_eq!(resp.headers()["content-type"], "application/json");
    let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .unwrap();
    assert_eq!(&bytes[..], body.as_bytes());
}

#[tokio::test]
async fn wellknown_unset_get_404() {
    let app = app().await;
    let resp = app
        .oneshot(
            Request::get("/.well-known/apple-app-site-association")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn wellknown_admin_get_unset_returns_200_empty() {
    let app = app_admin("secret").await;
    let resp = app
        .oneshot(
            Request::get("/admin/wellknown/assetlinks.json")
                .header("x-admin-token", "secret")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .unwrap();
    assert!(body.is_empty());
}

#[tokio::test]
async fn wellknown_put_non_json_400() {
    let app = app_admin("secret").await;
    let resp = app
        .oneshot(
            Request::put("/admin/wellknown/assetlinks.json")
                .header("x-admin-token", "secret")
                .body(Body::from("not json at all"))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn wellknown_put_bogus_name_404() {
    let app = app_admin("secret").await;
    let resp = app
        .oneshot(
            Request::put("/admin/wellknown/bogus")
                .header("x-admin-token", "secret")
                .body(Body::from("{}"))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn wellknown_put_too_large_400() {
    let app = app_admin("secret").await;
    let big = format!(r#"{{"x":"{}"}}"#, "a".repeat(70000));
    let resp = app
        .oneshot(
            Request::put("/admin/wellknown/assetlinks.json")
                .header("x-admin-token", "secret")
                .body(Body::from(big))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn wellknown_put_without_token_401() {
    let app = app_admin("secret").await;
    let resp = app
        .oneshot(
            Request::put("/admin/wellknown/assetlinks.json")
                .body(Body::from("{}"))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn wellknown_aasa_served_on_legacy_root() {
    let app = app_admin("secret").await;
    let body = r#"{"applinks":{"apps":[],"details":[]}}"#;
    let resp = app
        .clone()
        .oneshot(
            Request::put("/admin/wellknown/apple-app-site-association")
                .header("x-admin-token", "secret")
                .body(Body::from(body))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    let resp = app
        .oneshot(
            Request::get("/apple-app-site-association")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    assert_eq!(resp.headers()["content-type"], "application/json");
    let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .unwrap();
    assert_eq!(&bytes[..], body.as_bytes());
}

const IPHONE_UA: &str = "Mozilla/5.0 (iPhone; CPU iPhone OS 17_0 like Mac OS X)";
const ANDROID_UA: &str = "Mozilla/5.0 (Linux; Android 14; Pixel 8)";
const DESKTOP_UA: &str = "Mozilla/5.0 (Windows NT 10.0; Win64; x64)";

async fn post_code(app: &axum::Router, body: &str) -> String {
    let resp = app
        .clone()
        .oneshot(
            Request::post("/")
                .header("content-type", "application/json")
                .header("x-admin-token", "secret")
                .body(Body::from(body.to_string()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .unwrap();
    let v: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    v["code"].as_str().unwrap().to_string()
}

async fn location_for_ua(app: &axum::Router, code: &str, ua: &str) -> (StatusCode, String) {
    let resp = app
        .clone()
        .oneshot(
            Request::get(format!("/{code}"))
                .header("user-agent", ua)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let status = resp.status();
    let loc = resp
        .headers()
        .get("location")
        .map(|v| v.to_str().unwrap().to_string())
        .unwrap_or_default();
    (status, loc)
}

#[tokio::test]
async fn app_ios_destination_used_for_iphone_but_not_desktop() {
    let app = app().await;
    let code = post_code(
        &app,
        r#"{"url":"https://example.com","app_ios":"https://apps.apple.com/app/x"}"#,
    )
    .await;

    let (status, loc) = location_for_ua(&app, &code, IPHONE_UA).await;
    assert_eq!(status, StatusCode::FOUND);
    assert_eq!(loc, "https://apps.apple.com/app/x");

    let (status, loc) = location_for_ua(&app, &code, DESKTOP_UA).await;
    assert_eq!(status, StatusCode::FOUND);
    assert_eq!(loc, "https://example.com");
}

#[tokio::test]
async fn no_app_fields_redirects_to_url_regardless_of_ua() {
    let app = app().await;
    let code = post_code(&app, r#"{"url":"https://example.com"}"#).await;

    let (status, loc) = location_for_ua(&app, &code, IPHONE_UA).await;
    assert_eq!(status, StatusCode::FOUND);
    assert_eq!(loc, "https://example.com");

    let (status, loc) = location_for_ua(&app, &code, ANDROID_UA).await;
    assert_eq!(status, StatusCode::FOUND);
    assert_eq!(loc, "https://example.com");
}

#[tokio::test]
async fn create_internal_app_ios_403() {
    let app = app().await;
    let resp = app
        .oneshot(
            Request::post("/")
                .header("content-type", "application/json")
                .body(Body::from(
                    r#"{"url":"https://example.com","app_ios":"http://127.0.0.1/"}"#,
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    // An internal/self target is a policy denial, matching the main-url path: 403.
    assert_eq!(resp.status(), StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn create_malformed_app_ios_400() {
    let app = app().await;
    let resp = app
        .oneshot(
            Request::post("/")
                .header("content-type", "application/json")
                .body(Body::from(
                    r#"{"url":"https://example.com","app_ios":"ftp://example.com"}"#,
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    // A malformed URL (wrong scheme) is a bad request, not a policy denial: 400.
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn patch_internal_app_android_403() {
    let app = app_admin("secret").await;
    let code = create_and_get_code(&app, "https://ok.com").await;
    let r = app
        .oneshot(
            Request::patch(format!("/admin/links/{code}"))
                .header("x-admin-token", "secret")
                .header("content-type", "application/json")
                .body(Body::from(r#"{"app_android":"http://127.0.0.1/"}"#))
                .unwrap(),
        )
        .await
        .unwrap();
    // An internal/self target is a policy denial, matching the main-url path: 403.
    assert_eq!(r.status(), StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn patch_adds_app_android_used_for_android_ua() {
    let app = app_admin("secret").await;
    let code = create_and_get_code(&app, "https://example.com").await;
    let r = app
        .clone()
        .oneshot(
            Request::patch(format!("/admin/links/{code}"))
                .header("x-admin-token", "secret")
                .header("content-type", "application/json")
                .body(Body::from(
                    r#"{"app_android":"https://play.google.com/store/apps/x"}"#,
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(r.status(), StatusCode::OK);

    let (status, loc) = location_for_ua(&app, &code, ANDROID_UA).await;
    assert_eq!(status, StatusCode::FOUND);
    assert_eq!(loc, "https://play.google.com/store/apps/x");
}

#[tokio::test]
async fn cors_header_present_when_configured() {
    let dir = Box::leak(Box::new(tempfile::tempdir().unwrap()));
    let (store, sink) = open_backends(dir.path()).await.unwrap();
    let cache = Cache::new(store.clone(), 1000);
    let (tx, _rx) = tokio::sync::mpsc::channel(100);
    let store2 = store.clone();
    let state = Arc::new(AppState {
        cache,
        store,
        key: 0x1234,
        analytics_tx: tx,
        sink,
        admin_token: None,
        ratelimiter: quark::abuse::ratelimit::RateLimiter::disabled(),
        blocklist: quark::abuse::blocklist::Blocklist::new(store2, None, 60),
        block_private: true,
        public_host: None,
        real_ip_header: "cf-connecting-ip".into(),
        webhooks: test_webhook_dispatcher(),
    });
    let app = quark::api::router_with_cors(state, vec!["https://panel.example".into()]);
    let resp = app
        .oneshot(
            Request::get("/health")
                .header("origin", "https://panel.example")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(
        resp.headers().get("access-control-allow-origin").unwrap(),
        "https://panel.example"
    );
}

#[tokio::test]
async fn redirect_without_rules_goes_to_default_url() {
    let app = app().await;
    let resp = app
        .clone()
        .oneshot(
            Request::post("/")
                .header("content-type", "application/json")
                .body(Body::from(r#"{"url":"https://default.example"}"#))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .unwrap();
    let v: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    let code = v["code"].as_str().unwrap().to_string();

    let resp = app
        .oneshot(
            Request::get(format!("/{code}"))
                .header("cf-ipcountry", "BR")
                .header("user-agent", "Mozilla/5.0 (iPhone; CPU iPhone OS)")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::FOUND);
    assert_eq!(resp.headers()["location"], "https://default.example");
}

#[tokio::test]
async fn redirect_country_rule_matches_and_falls_back() {
    let app = app().await;
    let resp = app
        .clone()
        .oneshot(
            Request::post("/")
                .header("content-type", "application/json")
                .body(Body::from(
                    r#"{"url":"https://default.example","rules":[{"field":"country","values":["BR"],"to":"https://br.example"}]}"#,
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .unwrap();
    let v: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    let code = v["code"].as_str().unwrap().to_string();

    let resp = app
        .clone()
        .oneshot(
            Request::get(format!("/{code}"))
                .header("cf-ipcountry", "BR")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::FOUND);
    assert_eq!(resp.headers()["location"], "https://br.example");

    let resp = app
        .oneshot(
            Request::get(format!("/{code}"))
                .header("cf-ipcountry", "US")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::FOUND);
    assert_eq!(resp.headers()["location"], "https://default.example");
}

#[tokio::test]
async fn redirect_device_rule_matches_via_mobile_ua() {
    let app = app().await;
    let resp = app
        .clone()
        .oneshot(
            Request::post("/")
                .header("content-type", "application/json")
                .body(Body::from(
                    r#"{"url":"https://default.example","rules":[{"field":"device","values":["Mobile"],"to":"https://m.example"}]}"#,
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .unwrap();
    let v: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    let code = v["code"].as_str().unwrap().to_string();

    let resp = app
        .oneshot(
            Request::get(format!("/{code}"))
                .header("user-agent", "Mozilla/5.0 (iPhone; CPU iPhone OS)")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::FOUND);
    assert_eq!(resp.headers()["location"], "https://m.example");
}

#[tokio::test]
async fn redirect_first_matching_rule_wins() {
    let app = app().await;
    let resp = app
        .clone()
        .oneshot(
            Request::post("/")
                .header("content-type", "application/json")
                .body(Body::from(
                    r#"{"url":"https://default.example","rules":[
                        {"field":"country","values":["BR"],"to":"https://first.example"},
                        {"field":"country","values":["BR"],"to":"https://second.example"}
                    ]}"#,
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .unwrap();
    let v: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    let code = v["code"].as_str().unwrap().to_string();

    let resp = app
        .oneshot(
            Request::get(format!("/{code}"))
                .header("cf-ipcountry", "BR")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::FOUND);
    assert_eq!(resp.headers()["location"], "https://first.example");
}

#[tokio::test]
async fn create_with_rule_to_internal_host_400_ssrf() {
    let app = app().await;
    let resp = app
        .oneshot(
            Request::post("/")
                .header("content-type", "application/json")
                .body(Body::from(
                    r#"{"url":"https://default.example","rules":[{"field":"country","values":["BR"],"to":"http://127.0.0.1"}]}"#,
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    // Same guard as the main url: an internal host is FORBIDDEN, not a
    // generic BAD_REQUEST (see blocks_internal_destination_403 above).
    assert_eq!(resp.status(), StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn create_with_too_many_rules_400() {
    let app = app().await;
    let rules: Vec<String> = (0..21)
        .map(|i| format!(r#"{{"field":"country","values":["BR"],"to":"https://r{i}.example"}}"#))
        .collect();
    let body = format!(
        r#"{{"url":"https://default.example","rules":[{}]}}"#,
        rules.join(",")
    );
    let resp = app
        .oneshot(
            Request::post("/")
                .header("content-type", "application/json")
                .body(Body::from(body))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn admin_patch_rule_to_internal_host_400_ssrf() {
    let app = app_admin("secret").await;
    let code = create_and_get_code(&app, "https://ok.com").await;
    let r = app
        .oneshot(
            Request::patch(format!("/admin/links/{code}"))
                .header("x-admin-token", "secret")
                .header("content-type", "application/json")
                .body(Body::from(
                    r#"{"rules":[{"field":"country","values":["BR"],"to":"http://127.0.0.1"}]}"#,
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    // Same guard as the main url: an internal host is FORBIDDEN, not a
    // generic BAD_REQUEST.
    assert_eq!(r.status(), StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn admin_patch_rules_then_redirect_applies_them() {
    let app = app_admin("secret").await;
    let code = create_and_get_code(&app, "https://ok.com").await;
    let r = app
        .clone()
        .oneshot(
            Request::patch(format!("/admin/links/{code}"))
                .header("x-admin-token", "secret")
                .header("content-type", "application/json")
                .body(Body::from(
                    r#"{"rules":[{"field":"country","values":["br"],"to":"https://br.example"}]}"#,
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(r.status(), StatusCode::OK);

    let r = app
        .oneshot(
            Request::get(format!("/{code}"))
                .header("cf-ipcountry", "BR")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(r.status(), StatusCode::FOUND);
    assert_eq!(r.headers()["location"], "https://br.example");
}

#[tokio::test]
async fn admin_patch_with_too_many_rules_400() {
    let app = app_admin("secret").await;
    let code = create_and_get_code(&app, "https://ok.com").await;
    let rules: Vec<String> = (0..21)
        .map(|i| format!(r#"{{"field":"country","values":["BR"],"to":"https://r{i}.example"}}"#))
        .collect();
    let body = format!(r#"{{"rules":[{}]}}"#, rules.join(","));
    let r = app
        .oneshot(
            Request::patch(format!("/admin/links/{code}"))
                .header("x-admin-token", "secret")
                .header("content-type", "application/json")
                .body(Body::from(body))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(r.status(), StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn admin_patch_with_invalid_device_value_400() {
    let app = app_admin("secret").await;
    let code = create_and_get_code(&app, "https://ok.com").await;
    let r = app
        .oneshot(
            Request::patch(format!("/admin/links/{code}"))
                .header("x-admin-token", "secret")
                .header("content-type", "application/json")
                .body(Body::from(
                    r#"{"rules":[{"field":"device","values":["Tablet"],"to":"https://t.example"}]}"#,
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(r.status(), StatusCode::BAD_REQUEST);
}
