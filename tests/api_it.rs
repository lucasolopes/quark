use axum::body::Body;
use axum::http::{Request, StatusCode};
use quark::api::{router, AppState};
use quark::cache::Cache;
use quark::store::open_backends;
use std::sync::Arc;
use tower::ServiceExt; // oneshot

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
    });
    router(state)
}

#[tokio::test]
async fn cria_e_redireciona() {
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
    assert_eq!(resp.status(), StatusCode::FOUND); // 302
    assert_eq!(resp.headers()["location"], "https://example.com");
}

#[tokio::test]
async fn codigo_inexistente_404() {
    let app = app().await;
    let resp = app
        .oneshot(Request::get("/0000000").body(Body::empty()).unwrap())
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn alias_em_uso_409() {
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
async fn link_expirado_410() {
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
async fn alias_redireciona() {
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
    assert_eq!(resp.status(), StatusCode::FOUND); // 302
    assert_eq!(resp.headers()["location"], "https://alias.example.com");
}

#[tokio::test]
async fn alias_numerico_rejeitado() {
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
async fn bloqueia_destino_interno_403() {
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
async fn bloqueia_dominio_na_blocklist_403() {
    // helper dedicado que semeia a blocklist antes de montar o app
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
async fn rate_limit_429_apos_estourar() {
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
async fn redirect_sem_ttl_tem_cache_control_default() {
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
async fn redirect_com_ttl_tem_cache_control_limitado_pelo_ttl() {
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
        .expect("deve ser public, max-age=<n>")
        .parse()
        .expect("max-age deve ser numérico");
    assert!(
        max_age > 0 && max_age <= 100,
        "max-age fora do esperado: {max_age}"
    );
}

#[tokio::test]
async fn codigo_inexistente_404_tem_cache_control_no_store() {
    let app = app().await;
    let resp = app
        .oneshot(Request::get("/0000000").body(Body::empty()).unwrap())
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
    assert_eq!(resp.headers()["cache-control"], "no-store");
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
