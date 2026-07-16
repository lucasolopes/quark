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
    let (store, sink) = open_backends(dir.path(), false).await.unwrap();
    let cache = Cache::new(store.clone(), 1000, None);
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
        admin_token: None,
        ratelimiter: quark::abuse::ratelimit::RateLimiter::disabled(),
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
async fn list_returns_app_destinations() {
    // The panel's edit dialog fills its app-destination fields from the list
    // row, so the list must carry app_ios/app_android (like it already carries
    // rules/variants). Omitting them made the fields render blank on edit.
    let app = app_admin("secret").await;
    let resp = app
        .clone()
        .oneshot(
            Request::post("/")
                .header("content-type", "application/json")
                .header("x-admin-token", "secret")
                .body(Body::from(
                    r#"{"url":"https://example.com","app_ios":"https://apps.apple.com/app/id123","app_android":"https://play.google.com/store/apps/details?id=com.ex"}"#,
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

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
    let row = &v["links"][0];
    assert_eq!(row["app_ios"], "https://apps.apple.com/app/id123");
    assert_eq!(
        row["app_android"],
        "https://play.google.com/store/apps/details?id=com.ex"
    );
}

#[tokio::test]
async fn list_omits_absent_app_destinations() {
    // A link with no app destinations must not carry null/empty app fields
    // (skip_serializing_if keeps the common row lean).
    let app = app_admin("secret").await;
    let resp = app
        .clone()
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

    let resp = app
        .oneshot(
            Request::get("/admin/links?limit=10")
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
    let row = v["links"][0].as_object().unwrap();
    assert!(
        !row.contains_key("app_ios"),
        "absent app_ios must be omitted"
    );
    assert!(
        !row.contains_key("app_android"),
        "absent app_android must be omitted"
    );
}

#[tokio::test]
async fn create_with_folder_lists_row_and_folders() {
    let app = app_admin("secret").await;
    let resp = app
        .clone()
        .oneshot(
            Request::post("/")
                .header("content-type", "application/json")
                .header("x-admin-token", "secret")
                .body(Body::from(
                    r#"{"url":"https://example.com","folder":"Marketing"}"#,
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    let resp = app
        .clone()
        .oneshot(
            Request::get("/admin/links?limit=10")
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
    assert_eq!(v["links"][0]["folder"], "Marketing");

    let resp = app
        .oneshot(
            Request::get("/admin/folders")
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
    assert_eq!(v["folders"][0]["name"], "Marketing");
    assert_eq!(v["folders"][0]["count"], 1);
}

#[tokio::test]
async fn list_omits_absent_folder() {
    let app = app_admin("secret").await;
    let resp = app
        .clone()
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

    let resp = app
        .oneshot(
            Request::get("/admin/links?limit=10")
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
    let row = v["links"][0].as_object().unwrap();
    assert!(!row.contains_key("folder"), "absent folder must be omitted");
}

#[tokio::test]
async fn folder_filter_narrows_the_list() {
    let app = app_admin("secret").await;
    for url in [
        r#"{"url":"https://a.com","folder":"Marketing"}"#,
        r#"{"url":"https://b.com","folder":"Docs"}"#,
    ] {
        let resp = app
            .clone()
            .oneshot(
                Request::post("/")
                    .header("content-type", "application/json")
                    .header("x-admin-token", "secret")
                    .body(Body::from(url))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
    }

    let resp = app
        .oneshot(
            Request::get("/admin/links?limit=10&folder=marketing")
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
    let links = v["links"].as_array().unwrap();
    assert_eq!(links.len(), 1);
    assert_eq!(links[0]["folder"], "Marketing");
    assert_eq!(links[0]["url"], "https://a.com");
}

#[tokio::test]
async fn patch_folder_null_clears_it() {
    let app = app_admin("secret").await;
    let resp = app
        .clone()
        .oneshot(
            Request::post("/")
                .header("content-type", "application/json")
                .header("x-admin-token", "secret")
                .body(Body::from(
                    r#"{"url":"https://example.com","folder":"Marketing"}"#,
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .unwrap();
    let v: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    let code = v["code"].as_str().unwrap().to_string();

    let resp = app
        .clone()
        .oneshot(
            Request::patch(format!("/admin/links/{code}"))
                .header("content-type", "application/json")
                .header("x-admin-token", "secret")
                .body(Body::from(r#"{"folder":null}"#))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    let resp = app
        .oneshot(
            Request::get("/admin/links?limit=10")
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
    let row = v["links"][0].as_object().unwrap();
    assert!(
        !row.contains_key("folder"),
        "cleared folder must be omitted"
    );
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
async fn expired_link_with_fallback_redirects_302() {
    let app = app().await;
    let resp = app
        .clone()
        .oneshot(
            Request::post("/")
                .header("content-type", "application/json")
                .body(Body::from(
                    r#"{"url":"https://a.com","ttl":0,"fallback_url":"https://ended.example.com"}"#,
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
    assert_eq!(resp.headers()["location"], "https://ended.example.com");
    assert_eq!(resp.headers()["cache-control"], "no-store");
}

#[tokio::test]
async fn visit_exhausted_link_with_fallback_redirects_302() {
    let app = app_admin("secret").await;
    let resp = app
        .clone()
        .oneshot(
            Request::post("/")
                .header("content-type", "application/json")
                .header("x-admin-token", "secret")
                .body(Body::from(
                    r#"{"url":"https://example.com","max_visits":1,"fallback_url":"https://ended.example.com"}"#,
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

    // First visit consumes the single allowed visit and redirects normally.
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
    assert_eq!(r.headers()["location"], "https://example.com");

    // Second visit is over the limit: 302 to the fallback, not 410.
    let r = app
        .oneshot(
            Request::get(format!("/{code}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(r.status(), StatusCode::FOUND);
    assert_eq!(r.headers()["location"], "https://ended.example.com");
    assert_eq!(r.headers()["cache-control"], "no-store");
}

#[tokio::test]
async fn create_with_internal_fallback_url_rejected() {
    let app = app().await;
    let resp = app
        .oneshot(
            Request::post("/")
                .header("content-type", "application/json")
                .body(Body::from(
                    r#"{"url":"https://a.com","fallback_url":"http://127.0.0.1/x"}"#,
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    // Internal/self target is a policy denial: 403, same as the main URL guard.
    assert_eq!(resp.status(), StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn create_with_malformed_fallback_url_rejected() {
    let app = app().await;
    let resp = app
        .oneshot(
            Request::post("/")
                .header("content-type", "application/json")
                .body(Body::from(
                    r#"{"url":"https://a.com","fallback_url":"javascript:alert(1)"}"#,
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
}

/// Creates a password-protected link via the admin API and returns its code.
async fn create_protected(app: &axum::Router, url: &str, password: &str) -> String {
    let body = format!(r#"{{"url":"{url}","password":"{password}"}}"#);
    let resp = app
        .clone()
        .oneshot(
            Request::post("/")
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
    v["code"].as_str().unwrap().to_string()
}

#[tokio::test]
async fn protected_link_get_serves_interstitial() {
    let app = app_admin("secret").await;
    let code = create_protected(&app, "https://secret.example.com", "hunter2").await;

    let resp = app
        .oneshot(
            Request::get(format!("/{code}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    assert!(resp.headers()["content-type"]
        .to_str()
        .unwrap()
        .starts_with("text/html"));
    assert_eq!(resp.headers()["cache-control"], "no-store");
    let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .unwrap();
    let html = String::from_utf8(bytes.to_vec()).unwrap();
    assert!(html.contains(&format!(r#"action="/{code}""#)));
    assert!(html.contains(r#"name="password""#));
    // The destination URL must never appear in the interstitial.
    assert!(!html.contains("secret.example.com"));
}

#[tokio::test]
async fn protected_link_unlock_then_redirects() {
    let app = app_admin("secret").await;
    let code = create_protected(&app, "https://secret.example.com", "hunter2").await;

    // Correct password: 303 + Set-Cookie back to the link.
    let resp = app
        .clone()
        .oneshot(
            Request::post(format!("/{code}"))
                .header("content-type", "application/x-www-form-urlencoded")
                .body(Body::from("password=hunter2"))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::SEE_OTHER);
    assert_eq!(resp.headers()["location"], format!("/{code}"));
    let set_cookie = resp.headers()["set-cookie"].to_str().unwrap().to_string();
    let cookie = set_cookie.split(';').next().unwrap().to_string();
    assert!(cookie.starts_with(&format!("qk_pw_{code}=")));

    // Re-visiting with the unlock cookie skips the form and redirects.
    let resp = app
        .oneshot(
            Request::get(format!("/{code}"))
                .header("cookie", &cookie)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::FOUND);
    assert_eq!(resp.headers()["location"], "https://secret.example.com");
    // Regression: a protected link's redirect must never be cacheable, or a
    // shared CDN would serve it to visitors who never entered the password.
    assert_eq!(resp.headers()["cache-control"], "no-store");
}

#[tokio::test]
async fn protected_unlock_preserves_query_string() {
    let app = app_admin("secret").await;
    let code = create_protected(&app, "https://secret.example.com", "hunter2").await;

    // Regression: the query string (e.g. fbclid) must survive the unlock
    // round-trip, so attribution parity with unprotected links is kept.
    let resp = app
        .oneshot(
            Request::post(format!("/{code}?fbclid=abc123&x=1"))
                .header("content-type", "application/x-www-form-urlencoded")
                .body(Body::from("password=hunter2"))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::SEE_OTHER);
    assert_eq!(
        resp.headers()["location"],
        format!("/{code}?fbclid=abc123&x=1")
    );
}

#[tokio::test]
async fn rotating_password_invalidates_existing_unlock_cookie() {
    let app = app_admin("secret").await;
    let code = create_protected(&app, "https://secret.example.com", "hunter2").await;

    // Unlock and capture the cookie.
    let resp = app
        .clone()
        .oneshot(
            Request::post(format!("/{code}"))
                .header("content-type", "application/x-www-form-urlencoded")
                .body(Body::from("password=hunter2"))
                .unwrap(),
        )
        .await
        .unwrap();
    let set_cookie = resp.headers()["set-cookie"].to_str().unwrap().to_string();
    let cookie = set_cookie.split(';').next().unwrap().to_string();

    // Rotate the password.
    let resp = app
        .clone()
        .oneshot(
            Request::patch(format!("/admin/links/{code}"))
                .header("content-type", "application/json")
                .header("x-admin-token", "secret")
                .body(Body::from(r#"{"password":"newpass"}"#))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    // The old cookie must no longer unlock: the interstitial is shown again.
    let resp = app
        .oneshot(
            Request::get(format!("/{code}"))
                .header("cookie", &cookie)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    assert!(resp.headers()["content-type"]
        .to_str()
        .unwrap()
        .starts_with("text/html"));
}

#[tokio::test]
async fn protected_link_wrong_password_reprompts_without_cookie() {
    let app = app_admin("secret").await;
    let code = create_protected(&app, "https://secret.example.com", "hunter2").await;

    let resp = app
        .oneshot(
            Request::post(format!("/{code}"))
                .header("content-type", "application/x-www-form-urlencoded")
                .body(Body::from("password=wrong"))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    assert!(resp.headers().get("set-cookie").is_none());
    let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .unwrap();
    let html = String::from_utf8(bytes.to_vec()).unwrap();
    assert!(html.contains(r#"name="password""#));
}

#[tokio::test]
async fn admin_row_reports_has_password_without_leaking_hash() {
    let app = app_admin("secret").await;
    let _code = create_protected(&app, "https://secret.example.com", "hunter2").await;

    let resp = app
        .oneshot(
            Request::get("/admin/links")
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
    let row = &v["links"][0];
    assert_eq!(row["has_password"], true);
    assert!(
        row.get("password_hash").is_none(),
        "the hash must never be serialized"
    );
    // The plaintext/hash must not appear anywhere in the response.
    let raw = String::from_utf8(bytes.to_vec()).unwrap();
    assert!(!raw.contains("hunter2") && !raw.contains("argon2"));
}

#[tokio::test]
async fn patch_clears_password_reopening_the_link() {
    let app = app_admin("secret").await;
    let code = create_protected(&app, "https://secret.example.com", "hunter2").await;

    // Clearing the password with null reopens the link (no interstitial).
    let resp = app
        .clone()
        .oneshot(
            Request::patch(format!("/admin/links/{code}"))
                .header("content-type", "application/json")
                .header("x-admin-token", "secret")
                .body(Body::from(r#"{"password":null}"#))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    let resp = app
        .oneshot(
            Request::get(format!("/{code}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::FOUND);
    assert_eq!(resp.headers()["location"], "https://secret.example.com");
}

#[tokio::test]
async fn unlock_post_is_rate_limited() {
    let dir = Box::leak(Box::new(tempfile::tempdir().unwrap()));
    let (store, sink) = open_backends(dir.path(), false).await.unwrap();
    let cache = Cache::new(store.clone(), 1000, None);
    let (tx, _rx) = tokio::sync::mpsc::channel(100);
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
        analytics_tx: tx,
        sink,
        admin_token: Some("secret".to_string()),
        ratelimiter: quark::abuse::ratelimit::RateLimiter::memory(1),
        block_private: true,
        public_host: None,
        real_ip_header: "cf-connecting-ip".to_string(),
        webhooks: test_webhook_dispatcher(),
    });
    let app = router(state);
    let code = create_protected(&app, "https://secret.example.com", "hunter2").await;

    let mk = || {
        Request::post(format!("/{code}"))
            .header("content-type", "application/x-www-form-urlencoded")
            .header("cf-connecting-ip", "7.7.7.7")
            .body(Body::from("password=wrong"))
            .unwrap()
    };
    // First POST is allowed (re-prompts), second is over the per-minute cap.
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
async fn rate_limit_429_after_exceeding() {
    let dir = Box::leak(Box::new(tempfile::tempdir().unwrap()));
    let (store, sink) = open_backends(dir.path(), false).await.unwrap();
    let cache = Cache::new(store.clone(), 1000, None);
    let (tx, _rx) = tokio::sync::mpsc::channel(100);
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
        analytics_tx: tx,
        sink,
        admin_token: None,
        ratelimiter: quark::abuse::ratelimit::RateLimiter::memory(1),
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
    let (store, sink) = open_backends(dir.path(), false).await.unwrap();
    let cache = Cache::new(store.clone(), 1000, None);
    let (tx, _rx) = tokio::sync::mpsc::channel(100);
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
        analytics_tx: tx,
        sink,
        admin_token: Some(token.to_string()),
        ratelimiter: quark::abuse::ratelimit::RateLimiter::disabled(),
        block_private: true,
        public_host: None,
        real_ip_header: "cf-connecting-ip".to_string(),
        webhooks: test_webhook_dispatcher(),
    });
    router(state)
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
async fn admin_tags_returns_names_with_counts() {
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
    // Sorted by name: cli(1), rust(1), web(2).
    let rows = v["tags"].as_array().unwrap();
    let pairs: Vec<(&str, u64)> = rows
        .iter()
        .map(|t| (t["name"].as_str().unwrap(), t["count"].as_u64().unwrap()))
        .collect();
    assert_eq!(pairs, vec![("cli", 1), ("rust", 1), ("web", 2)]);
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

/// P1b end-to-end: an admin-gated create (through `require_admin_for_create`
/// -> `admin_guard` -> `Principal`) followed by a plain read of the same
/// code. This is the behavior-preserving assertion for the whole P1b auth
/// chain: the write lands under the resolved `Principal`'s tenant
/// (`DEFAULT_TENANT` in P1b) and the redirect read finds it there, exactly
/// like the pre-P1b (untenanted) shortener did.
#[tokio::test]
async fn admin_gated_create_then_link_is_readable_default_tenant() {
    let app = app_admin("secret").await;

    let resp = app
        .clone()
        .oneshot(
            Request::post("/")
                .header("content-type", "application/json")
                .header("x-admin-token", "secret")
                .body(Body::from(r#"{"url":"https://example.com/p1b-e2e"}"#))
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
    assert_eq!(resp.headers()["location"], "https://example.com/p1b-e2e");
}

/// Builds an app like `app()` but returns the analytics receiver too, so
/// tests can inspect the `ClickEvent` (in particular `variant`) that the
/// redirect handler sends.
async fn app_with_analytics_rx() -> (axum::Router, tokio::sync::mpsc::Receiver<ClickEvent>) {
    let dir = Box::leak(Box::new(tempfile::tempdir().unwrap()));
    let (store, sink) = open_backends(dir.path(), false).await.unwrap();
    let cache = Cache::new(store.clone(), 1000, None);
    let (analytics_tx, rx) = tokio::sync::mpsc::channel(100);
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
        admin_token: None,
        ratelimiter: quark::abuse::ratelimit::RateLimiter::disabled(),
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
    let (store, sink) = open_backends(dir.path(), false).await.unwrap();
    let cache = Cache::new(store.clone(), 1000, None);
    let (tx, _rx) = tokio::sync::mpsc::channel(100);
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
        analytics_tx: tx,
        sink,
        admin_token: None,
        ratelimiter: quark::abuse::ratelimit::RateLimiter::disabled(),
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

#[tokio::test]
async fn admin_links_reports_health_and_broken_filter() {
    let dir = Box::leak(Box::new(tempfile::tempdir().unwrap()));
    let (store, sink) = open_backends(dir.path(), false).await.unwrap();
    let cache = Cache::new(store.clone(), 1000, None);
    let (tx, _rx) = tokio::sync::mpsc::channel(100);
    let state = Arc::new(AppState {
        oidc: None,
        sheets: None,
        sheets_api: None,
        oidc_configured: false,
        multi_tenant: false,
        cache,
        store: store.clone(),
        key: 0x1234,
        signing_key: [0u8; 32],
        analytics_tx: tx,
        sink,
        admin_token: Some("secret".to_string()),
        ratelimiter: quark::abuse::ratelimit::RateLimiter::disabled(),
        block_private: true,
        public_host: None,
        real_ip_header: "cf-connecting-ip".to_string(),
        webhooks: test_webhook_dispatcher(),
    });
    let app = router(state);

    let mk = |url: &str| {
        Request::post("/")
            .header("content-type", "application/json")
            .header("x-admin-token", "secret")
            .body(Body::from(format!(r#"{{"url":"{url}"}}"#)))
            .unwrap()
    };
    app.clone()
        .oneshot(mk("https://ok.example.com"))
        .await
        .unwrap();
    app.clone()
        .oneshot(mk("https://dead.example.com"))
        .await
        .unwrap();

    // Read back the two ids assigned by the store.
    let list = |q: &str| {
        Request::get(format!("/admin/links{q}"))
            .header("x-admin-token", "secret")
            .body(Body::empty())
            .unwrap()
    };
    let resp = app.clone().oneshot(list("")).await.unwrap();
    let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .unwrap();
    let v: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    let rows = v["links"].as_array().unwrap();
    assert_eq!(rows.len(), 2);
    // No health recorded yet: the field is omitted.
    assert!(rows.iter().all(|r| r.get("health").is_none()));
    let id_by_url = |url: &str| -> u64 {
        rows.iter().find(|r| r["url"] == url).unwrap()["id"]
            .as_u64()
            .unwrap()
    };
    let ok_id = id_by_url("https://ok.example.com");
    let dead_id = id_by_url("https://dead.example.com");

    store
        .put_link_health(
            quark::tenant::DEFAULT_TENANT,
            ok_id,
            &quark::store::LinkHealth {
                checked_at: 10,
                status: Some(200),
                healthy: true,
            },
        )
        .await
        .unwrap();
    store
        .put_link_health(
            quark::tenant::DEFAULT_TENANT,
            dead_id,
            &quark::store::LinkHealth {
                checked_at: 10,
                status: Some(404),
                healthy: false,
            },
        )
        .await
        .unwrap();

    // Unfiltered list now carries health on both rows.
    let resp = app.clone().oneshot(list("")).await.unwrap();
    let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .unwrap();
    let v: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    let rows = v["links"].as_array().unwrap();
    let health_of =
        |id: u64| rows.iter().find(|r| r["id"].as_u64() == Some(id)).unwrap()["health"].clone();
    assert_eq!(health_of(ok_id)["healthy"], true);
    assert_eq!(health_of(dead_id)["healthy"], false);
    assert_eq!(health_of(dead_id)["status"], 404);

    // ?health=broken narrows to the dead link only.
    let resp = app.oneshot(list("?health=broken")).await.unwrap();
    let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .unwrap();
    let v: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    let rows = v["links"].as_array().unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0]["id"].as_u64(), Some(dead_id));
    assert_eq!(rows[0]["health"]["healthy"], false);
}

#[tokio::test]
async fn session_cookie_authorizes_admin_by_scope() {
    use quark::auth::{hash_token, Scope, Session};
    let dir = Box::leak(Box::new(tempfile::tempdir().unwrap()));
    let (store, sink) = open_backends(dir.path(), false).await.unwrap();
    let cache = Cache::new(store.clone(), 1000, None);
    let (tx, _rx) = tokio::sync::mpsc::channel(100);
    let state = Arc::new(AppState {
        oidc: None,
        sheets: None,
        sheets_api: None,
        oidc_configured: true,
        multi_tenant: false,
        cache,
        store: store.clone(),
        key: 0x1234,
        signing_key: [0u8; 32],
        analytics_tx: tx,
        sink,
        admin_token: Some("secret".to_string()),
        ratelimiter: quark::abuse::ratelimit::RateLimiter::disabled(),
        block_private: true,
        public_host: None,
        real_ip_header: "cf-connecting-ip".to_string(),
        webhooks: test_webhook_dispatcher(),
    });
    let app = router(state);
    let now = 1_000_000u64;
    // A reader session (links_read + analytics) can list links.
    store
        .put_session(
            quark::tenant::DEFAULT_TENANT,
            &Session {
                token_hash: hash_token("reader-token"),
                subject: "s1".into(),
                display: "reader@example.com".into(),
                scopes: vec![Scope::LinksRead, Scope::Analytics],
                created: now,
                expires: now + 100_000_000_000,
                tenant_id: quark::tenant::DEFAULT_TENANT,
                user_id: 0,
            },
        )
        .await
        .unwrap();
    let get = |cookie: Option<&str>| {
        let mut req = Request::get("/admin/links");
        if let Some(c) = cookie {
            req = req.header("cookie", format!("qk_session={c}"));
        }
        req.body(Body::empty()).unwrap()
    };
    // Valid reader cookie -> 200.
    assert_eq!(
        app.clone()
            .oneshot(get(Some("reader-token")))
            .await
            .unwrap()
            .status(),
        StatusCode::OK
    );
    // No cookie, no token -> 401 (admin enabled, unauthenticated).
    assert_eq!(
        app.clone().oneshot(get(None)).await.unwrap().status(),
        StatusCode::UNAUTHORIZED
    );
    // Unknown cookie -> 401.
    assert_eq!(
        app.clone()
            .oneshot(get(Some("nope")))
            .await
            .unwrap()
            .status(),
        StatusCode::UNAUTHORIZED
    );

    // A webhooks-only session cannot write links (PATCH needs links_write) -> 403.
    store
        .put_session(
            quark::tenant::DEFAULT_TENANT,
            &Session {
                token_hash: hash_token("hooks-token"),
                subject: "s2".into(),
                display: "hooks@example.com".into(),
                scopes: vec![Scope::Webhooks],
                created: now,
                expires: now + 100_000_000_000,
                tenant_id: quark::tenant::DEFAULT_TENANT,
                user_id: 0,
            },
        )
        .await
        .unwrap();
    let resp = app
        .clone()
        .oneshot(
            Request::patch("/admin/links/whatever")
                .header("cookie", "qk_session=hooks-token")
                .header("content-type", "application/json")
                .body(Body::from(r#"{"url":"https://x.com"}"#))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::FORBIDDEN);

    // An expired session does not authenticate -> 401.
    store
        .put_session(
            quark::tenant::DEFAULT_TENANT,
            &Session {
                token_hash: hash_token("stale-token"),
                subject: "s3".into(),
                display: "stale@example.com".into(),
                scopes: vec![Scope::Full],
                created: 1,
                expires: 2,
                tenant_id: quark::tenant::DEFAULT_TENANT,
                user_id: 0,
            },
        )
        .await
        .unwrap();
    assert_eq!(
        app.clone()
            .oneshot(get(Some("stale-token")))
            .await
            .unwrap()
            .status(),
        StatusCode::UNAUTHORIZED
    );
}

#[tokio::test]
async fn admin_me_reports_session_and_oidc_state() {
    use quark::auth::{hash_token, Scope, Session};
    let dir = Box::leak(Box::new(tempfile::tempdir().unwrap()));
    let (store, sink) = open_backends(dir.path(), false).await.unwrap();
    let cache = Cache::new(store.clone(), 1000, None);
    let (tx, _rx) = tokio::sync::mpsc::channel(100);
    let state = Arc::new(AppState {
        oidc: None,
        sheets: None,
        sheets_api: None,
        oidc_configured: false,
        multi_tenant: false,
        cache,
        store: store.clone(),
        key: 0x1234,
        signing_key: [0u8; 32],
        analytics_tx: tx,
        sink,
        admin_token: Some("secret".to_string()),
        ratelimiter: quark::abuse::ratelimit::RateLimiter::disabled(),
        block_private: true,
        public_host: None,
        real_ip_header: "cf-connecting-ip".to_string(),
        webhooks: test_webhook_dispatcher(),
    });
    let app = router(state);

    // No session -> authenticated:false, oidc_enabled:false.
    let resp = app
        .clone()
        .oneshot(Request::get("/admin/me").body(Body::empty()).unwrap())
        .await
        .unwrap();
    let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .unwrap();
    let v: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(v["authenticated"], false);
    assert_eq!(v["oidc_enabled"], false);

    store
        .put_session(
            quark::tenant::DEFAULT_TENANT,
            &Session {
                token_hash: hash_token("tok"),
                subject: "s1".into(),
                display: "me@example.com".into(),
                scopes: vec![Scope::Full],
                created: 1,
                expires: 100_000_000_000,
                tenant_id: quark::tenant::DEFAULT_TENANT,
                user_id: 0,
            },
        )
        .await
        .unwrap();
    let resp = app
        .oneshot(
            Request::get("/admin/me")
                .header("cookie", "qk_session=tok")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .unwrap();
    let v: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(v["authenticated"], true);
    assert_eq!(v["display"], "me@example.com");
}

#[tokio::test]
async fn login_route_404_when_oidc_disabled() {
    let app = app_admin("secret").await;
    let resp = app
        .oneshot(Request::get("/admin/login").body(Body::empty()).unwrap())
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn oidc_session_can_create_and_low_scope_token_does_not_block_it() {
    use quark::auth::{hash_token, ApiToken, Scope, Session};
    let dir = Box::leak(Box::new(tempfile::tempdir().unwrap()));
    let (store, sink) = open_backends(dir.path(), false).await.unwrap();
    let cache = Cache::new(store.clone(), 1000, None);
    let (tx, _rx) = tokio::sync::mpsc::channel(100);
    let state = Arc::new(AppState {
        oidc: None,
        sheets: None,
        sheets_api: None,
        oidc_configured: true,
        multi_tenant: false,
        cache,
        store: store.clone(),
        key: 0x1234,
        signing_key: [0u8; 32],
        analytics_tx: tx,
        sink,
        admin_token: Some("secret".to_string()),
        ratelimiter: quark::abuse::ratelimit::RateLimiter::disabled(),
        block_private: true,
        public_host: None,
        real_ip_header: "cf-connecting-ip".to_string(),
        webhooks: test_webhook_dispatcher(),
    });
    let app = router(state);
    let now = 1_000_000u64;

    // A Full session created via OIDC can create a link (POST /) with only the
    // session cookie (no x-admin-token) — the previous bug bounced it with 401.
    store
        .put_session(
            quark::tenant::DEFAULT_TENANT,
            &Session {
                token_hash: hash_token("full-sess"),
                subject: "admin".into(),
                display: "admin@example.com".into(),
                scopes: vec![Scope::Full],
                created: now,
                expires: now + 100_000_000_000,
                tenant_id: quark::tenant::DEFAULT_TENANT,
                user_id: 0,
            },
        )
        .await
        .unwrap();
    let resp = app
        .clone()
        .oneshot(
            Request::post("/")
                .header("content-type", "application/json")
                .header("cookie", "qk_session=full-sess")
                .body(Body::from(r#"{"url":"https://ok.com"}"#))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(
        resp.status(),
        StatusCode::OK,
        "OIDC session should authorize create"
    );

    // A low-scope API token in x-admin-token must NOT block a sufficiently-scoped
    // session: send both, expect the session to authorize a links_read GET.
    store
        .put_api_token(
            quark::tenant::DEFAULT_TENANT,
            &ApiToken {
                id: 1,
                name: "readonly".into(),
                token_hash: hash_token("weak-token"),
                scopes: vec![Scope::Webhooks],
                rate_limit_per_min: None,
                created: now,
                tenant_id: quark::tenant::DEFAULT_TENANT,
            },
        )
        .await
        .unwrap();
    store
        .put_session(
            quark::tenant::DEFAULT_TENANT,
            &Session {
                token_hash: hash_token("reader-sess"),
                subject: "reader".into(),
                display: "r@example.com".into(),
                scopes: vec![Scope::LinksRead],
                created: now,
                expires: now + 100_000_000_000,
                tenant_id: quark::tenant::DEFAULT_TENANT,
                user_id: 0,
            },
        )
        .await
        .unwrap();
    let resp = app
        .oneshot(
            Request::get("/admin/links")
                .header("x-admin-token", "weak-token")
                .header("cookie", "qk_session=reader-sess")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(
        resp.status(),
        StatusCode::OK,
        "insufficient token must not block a sufficient session"
    );
}

#[tokio::test]
async fn logout_requires_csrf_header_and_revokes_session() {
    use quark::auth::{hash_token, Scope, Session};
    let dir = Box::leak(Box::new(tempfile::tempdir().unwrap()));
    let (store, sink) = open_backends(dir.path(), false).await.unwrap();
    let cache = Cache::new(store.clone(), 1000, None);
    let (tx, _rx) = tokio::sync::mpsc::channel(100);
    let state = Arc::new(AppState {
        oidc: None,
        sheets: None,
        sheets_api: None,
        oidc_configured: true,
        multi_tenant: false,
        cache,
        store: store.clone(),
        key: 0x1234,
        signing_key: [0u8; 32],
        analytics_tx: tx,
        sink,
        admin_token: Some("secret".to_string()),
        ratelimiter: quark::abuse::ratelimit::RateLimiter::disabled(),
        block_private: true,
        public_host: None,
        real_ip_header: "cf-connecting-ip".to_string(),
        webhooks: test_webhook_dispatcher(),
    });
    let app = router(state);
    store
        .put_session(
            quark::tenant::DEFAULT_TENANT,
            &Session {
                token_hash: hash_token("sess"),
                subject: "s".into(),
                display: "s@example.com".into(),
                scopes: vec![Scope::Full],
                created: 1,
                expires: 100_000_000_000,
                tenant_id: quark::tenant::DEFAULT_TENANT,
                user_id: 0,
            },
        )
        .await
        .unwrap();

    // Without the CSRF header (a cross-site simple POST) -> 403, session kept.
    let resp = app
        .clone()
        .oneshot(
            Request::post("/admin/logout")
                .header("cookie", "qk_session=sess")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::FORBIDDEN);
    assert!(store
        .get_session_by_hash(&hash_token("sess"), 2)
        .await
        .unwrap()
        .is_some());

    // With the header (the panel's request) -> 204, session revoked.
    let resp = app
        .oneshot(
            Request::post("/admin/logout")
                .header("cookie", "qk_session=sess")
                .header("x-quark-csrf", "1")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::NO_CONTENT);
    assert!(store
        .get_session_by_hash(&hash_token("sess"), 2)
        .await
        .unwrap()
        .is_none());
}

#[tokio::test]
async fn session_cookie_is_ignored_when_oidc_not_configured() {
    use quark::auth::{hash_token, Scope, Session};
    let dir = Box::leak(Box::new(tempfile::tempdir().unwrap()));
    let (store, sink) = open_backends(dir.path(), false).await.unwrap();
    let cache = Cache::new(store.clone(), 1000, None);
    let (tx, _rx) = tokio::sync::mpsc::channel(100);
    let state = Arc::new(AppState {
        oidc: None,
        sheets: None,
        sheets_api: None,
        // OIDC turned off (e.g. QUARK_OIDC_ISSUER unset) while a token stays set.
        oidc_configured: false,
        multi_tenant: false,
        cache,
        store: store.clone(),
        key: 0x1234,
        signing_key: [0u8; 32],
        analytics_tx: tx,
        sink,
        admin_token: Some("secret".to_string()),
        ratelimiter: quark::abuse::ratelimit::RateLimiter::disabled(),
        block_private: true,
        public_host: None,
        real_ip_header: "cf-connecting-ip".to_string(),
        webhooks: test_webhook_dispatcher(),
    });
    let app = router(state);
    // A previously issued, still-unexpired full-scope session.
    store
        .put_session(
            quark::tenant::DEFAULT_TENANT,
            &Session {
                token_hash: hash_token("leftover"),
                subject: "s".into(),
                display: "s@example.com".into(),
                scopes: vec![Scope::Full],
                created: 1,
                expires: 100_000_000_000,
                tenant_id: quark::tenant::DEFAULT_TENANT,
                user_id: 0,
            },
        )
        .await
        .unwrap();

    // The leftover session must NOT authorize now that OIDC is off -> 401.
    let resp = app
        .clone()
        .oneshot(
            Request::get("/admin/links")
                .header("cookie", "qk_session=leftover")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);

    // The break-glass token still works.
    let resp = app
        .oneshot(
            Request::get("/admin/links")
                .header("x-admin-token", "secret")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
}

#[tokio::test]
async fn sheets_status_reports_connected_and_never_leaks_refresh_token() {
    use quark::auth::{hash_token, Scope, Session};
    let dir = Box::leak(Box::new(tempfile::tempdir().unwrap()));
    let (store, sink) = open_backends(dir.path(), false).await.unwrap();
    let cache = Cache::new(store.clone(), 1000, None);
    let (tx, _rx) = tokio::sync::mpsc::channel(100);
    let cfg = quark::sheets::SheetsConfig::from_parts(
        "cid",
        "sec",
        "https://h/admin/integrations/sheets/callback",
        None,
    )
    .unwrap();
    let state = Arc::new(AppState {
        oidc: None,
        sheets: Some(Arc::new(cfg)),
        sheets_api: None,
        oidc_configured: true,
        multi_tenant: false,
        cache,
        store: store.clone(),
        key: 0x1234,
        signing_key: [0u8; 32],
        analytics_tx: tx,
        sink,
        admin_token: Some("secret".to_string()),
        ratelimiter: quark::abuse::ratelimit::RateLimiter::disabled(),
        block_private: true,
        public_host: None,
        real_ip_header: "cf-connecting-ip".to_string(),
        webhooks: test_webhook_dispatcher(),
    });
    // Seed a connection whose refresh token must never appear in a response.
    store
        .put_sheets_connection(
            quark::tenant::DEFAULT_TENANT,
            &quark::sheets::SheetsConnection {
                refresh_token: "SECRET".into(),
                email: "op@example.com".into(),
                spreadsheet_id: Some("sheet123".into()),
                last_sync: Some(42),
                last_status: quark::sheets::SyncStatus::Ok,
            },
        )
        .await
        .unwrap();
    let app = router(state);

    // Status reports connected and never leaks the refresh token.
    let resp = app
        .clone()
        .oneshot(
            Request::get("/admin/integrations/sheets/status")
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
    let text = String::from_utf8(bytes.to_vec()).unwrap();
    assert!(
        !text.contains("SECRET"),
        "response must not leak refresh token"
    );
    let v: serde_json::Value = serde_json::from_str(&text).unwrap();
    assert_eq!(v["connected"], true);
    assert_eq!(v["email"], "op@example.com");
    assert_eq!(
        v["spreadsheet_url"],
        "https://docs.google.com/spreadsheets/d/sheet123"
    );

    // A session-cookie sync without the CSRF header is rejected (403), even
    // though the session authorizes the Full scope.
    let now = 1_000_000u64;
    store
        .put_session(
            quark::tenant::DEFAULT_TENANT,
            &Session {
                token_hash: hash_token("full-token"),
                subject: "s1".into(),
                display: "op@example.com".into(),
                scopes: vec![Scope::Full],
                created: now,
                expires: now + 100_000_000_000,
                tenant_id: quark::tenant::DEFAULT_TENANT,
                user_id: 0,
            },
        )
        .await
        .unwrap();
    let resp = app
        .oneshot(
            Request::post("/admin/integrations/sheets/sync")
                .header("cookie", "qk_session=full-token")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::FORBIDDEN);
}

// Regression for the connect-state binding: /connect sets a signed state cookie,
// and the callback refuses a state that is not backed by the matching cookie
// (anti login-CSRF: a leaked/echoed state alone must not connect an account).
#[tokio::test]
async fn sheets_callback_requires_the_state_cookie() {
    let dir = Box::leak(Box::new(tempfile::tempdir().unwrap()));
    let (store, sink) = open_backends(dir.path(), false).await.unwrap();
    let cache = Cache::new(store.clone(), 1000, None);
    let (tx, _rx) = tokio::sync::mpsc::channel(100);
    let cfg = quark::sheets::SheetsConfig::from_parts(
        "cid",
        "sec",
        "https://h/admin/integrations/sheets/callback",
        None,
    )
    .unwrap();
    let state = Arc::new(AppState {
        oidc: None,
        sheets: Some(Arc::new(cfg)),
        sheets_api: None,
        oidc_configured: true,
        multi_tenant: false,
        cache,
        store: store.clone(),
        key: 0x1234,
        signing_key: [0u8; 32],
        analytics_tx: tx,
        sink,
        admin_token: Some("secret".to_string()),
        ratelimiter: quark::abuse::ratelimit::RateLimiter::disabled(),
        block_private: true,
        public_host: None,
        real_ip_header: "cf-connecting-ip".to_string(),
        webhooks: test_webhook_dispatcher(),
    });
    let app = router(state);

    // Connect (admin-authed) returns the consent URL and sets the state cookie.
    let resp = app
        .clone()
        .oneshot(
            Request::get("/admin/integrations/sheets/connect")
                .header("x-admin-token", "secret")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let set_cookie = resp
        .headers()
        .get(axum::http::header::SET_COOKIE)
        .expect("connect sets a state cookie")
        .to_str()
        .unwrap()
        .to_string();
    assert!(set_cookie.contains("qk_sheets_state="));

    // A callback carrying a forged/echoed state but NO matching cookie is refused
    // (400), so it never reaches the token exchange and cannot store a connection.
    let resp = app
        .oneshot(
            Request::get("/admin/integrations/sheets/callback?code=x&state=anything")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    assert!(store
        .get_sheets_connection(quark::tenant::DEFAULT_TENANT)
        .await
        .unwrap()
        .is_none());
}

/// P2a Task 3: `/connect` must bind the state cookie to the CALLING
/// principal's tenant (not always `DEFAULT_TENANT`), so `sheets_callback` can
/// later persist the connection under the right tenant even though the
/// callback itself carries no admin credential. Exercised with an API token
/// scoped to a non-default tenant (P2b has no real tenant signup yet, but the
/// `Principal`/`ApiToken` plumbing already carries an arbitrary `tenant_id`).
#[tokio::test]
async fn sheets_connect_binds_the_state_cookie_to_the_callers_tenant() {
    use quark::auth::{hash_token, ApiToken, Scope};
    let dir = Box::leak(Box::new(tempfile::tempdir().unwrap()));
    let (store, sink) = open_backends(dir.path(), false).await.unwrap();
    let cache = Cache::new(store.clone(), 1000, None);
    let (tx, _rx) = tokio::sync::mpsc::channel(100);
    let cfg = quark::sheets::SheetsConfig::from_parts(
        "cid",
        "sec",
        "https://h/admin/integrations/sheets/callback",
        None,
    )
    .unwrap();
    let signing_key = [0u8; 32];
    let state = Arc::new(AppState {
        oidc: None,
        sheets: Some(Arc::new(cfg)),
        sheets_api: None,
        oidc_configured: false,
        multi_tenant: false,
        cache,
        store: store.clone(),
        key: 0x1234,
        signing_key,
        analytics_tx: tx,
        sink,
        admin_token: None,
        ratelimiter: quark::abuse::ratelimit::RateLimiter::disabled(),
        block_private: true,
        public_host: None,
        real_ip_header: "cf-connecting-ip".to_string(),
        webhooks: test_webhook_dispatcher(),
    });
    let tenant = quark::tenant::TenantId(42);
    store
        .put_api_token(
            tenant,
            &ApiToken {
                id: 1,
                name: "t".into(),
                token_hash: hash_token("tenant-42-token"),
                scopes: vec![Scope::Full],
                rate_limit_per_min: None,
                created: 0,
                tenant_id: tenant,
            },
        )
        .await
        .unwrap();
    let app = router(state);

    let resp = app
        .oneshot(
            Request::get("/admin/integrations/sheets/connect")
                .header("x-admin-token", "tenant-42-token")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let set_cookie = resp
        .headers()
        .get(axum::http::header::SET_COOKIE)
        .expect("connect sets a state cookie")
        .to_str()
        .unwrap()
        .to_string();
    let cookie_value = set_cookie
        .split(';')
        .next()
        .unwrap()
        .strip_prefix("qk_sheets_state=")
        .expect("cookie carries the sheets state name");
    let (_state, verifier, _nonce) = quark::oidc::verify_login_state(&signing_key, cookie_value)
        .expect("the cookie's HMAC must verify");
    assert_eq!(
        verifier,
        tenant.0.to_string(),
        "the state cookie must carry the calling principal's tenant"
    );
}
