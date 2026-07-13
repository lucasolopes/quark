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
    });
    router(state)
}

#[tokio::test]
async fn admin_blocklist_add_list_e_bloqueia() {
    let app = app_admin("segredo").await;
    // add
    let resp = app
        .clone()
        .oneshot(
            Request::post("/admin/blocklist")
                .header("content-type", "application/json")
                .header("x-admin-token", "segredo")
                .body(Body::from(r#"{"domain":"evil.com"}"#))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    // list contém
    let resp = app
        .clone()
        .oneshot(
            Request::get("/admin/blocklist")
                .header("x-admin-token", "segredo")
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
async fn admin_blocklist_sem_token_404() {
    // app() tem admin_token: None
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
async fn admin_blocklist_token_errado_401() {
    let app = app_admin("segredo").await;
    let resp = app
        .oneshot(
            Request::get("/admin/blocklist")
                .header("x-admin-token", "errado")
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
async fn admin_blocklist_sem_token_corpo_malformado_404() {
    // endpoint opaco: sem QUARK_ADMIN_TOKEN, ate POST com corpo ruim da 404 (nao 400/415)
    let app = app().await; // admin_token: None
    let resp = app
        .oneshot(
            Request::post("/admin/blocklist")
                .header("content-type", "application/json")
                .body(Body::from("nao eh json"))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn admin_blocklist_delete_remove() {
    let app = app_admin("segredo").await;
    // add
    app.clone()
        .oneshot(
            Request::post("/admin/blocklist")
                .header("content-type", "application/json")
                .header("x-admin-token", "segredo")
                .body(Body::from(r#"{"domain":"del.com"}"#))
                .unwrap(),
        )
        .await
        .unwrap();
    // delete
    let resp = app
        .clone()
        .oneshot(
            Request::delete("/admin/blocklist")
                .header("content-type", "application/json")
                .header("x-admin-token", "segredo")
                .body(Body::from(r#"{"domain":"del.com"}"#))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    // list vazia
    let resp = app
        .oneshot(
            Request::get("/admin/blocklist")
                .header("x-admin-token", "segredo")
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
async fn admin_links_lista_paginada() {
    let app = app_admin("segredo").await;
    // cria 2 links
    for u in ["https://a.com", "https://b.com"] {
        app.clone()
            .oneshot(
                Request::post("/")
                    .header("content-type", "application/json")
                    .header("x-admin-token", "segredo")
                    .body(Body::from(format!(r#"{{"url":"{u}"}}"#)))
                    .unwrap(),
            )
            .await
            .unwrap();
    }
    // lista
    let resp = app
        .oneshot(
            Request::get("/admin/links?limit=10")
                .header("x-admin-token", "segredo")
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
}

#[tokio::test]
async fn admin_links_sem_token_404() {
    let app = app().await; // admin_token: None
    let resp = app
        .oneshot(Request::get("/admin/links").body(Body::empty()).unwrap())
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
}

async fn cria_e_pega_code(app: &axum::Router, url: &str) -> String {
    let resp = app
        .clone()
        .oneshot(
            Request::post("/")
                .header("content-type", "application/json")
                .header("x-admin-token", "segredo")
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
async fn admin_delete_link_vira_404_no_redirect() {
    let app = app_admin("segredo").await;
    let code = cria_e_pega_code(&app, "https://del.com").await;
    // antes: redireciona
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
    // delete
    let r = app
        .clone()
        .oneshot(
            Request::delete(format!("/admin/links/{code}"))
                .header("x-admin-token", "segredo")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(r.status(), StatusCode::OK);
    // depois: 404
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
async fn admin_patch_link_atualiza_destino() {
    let app = app_admin("segredo").await;
    let code = cria_e_pega_code(&app, "https://velho.com").await;
    let r = app
        .clone()
        .oneshot(
            Request::patch(format!("/admin/links/{code}"))
                .header("x-admin-token", "segredo")
                .header("content-type", "application/json")
                .body(Body::from(r#"{"url":"https://novo.com"}"#))
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
    assert_eq!(r.headers()["location"], "https://novo.com");
}

#[tokio::test]
async fn admin_delete_inexistente_404() {
    let app = app_admin("segredo").await;
    let r = app
        .oneshot(
            Request::delete("/admin/links/0000000")
                .header("x-admin-token", "segredo")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(r.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn admin_patch_destino_interno_403() {
    let app = app_admin("segredo").await;
    let code = cria_e_pega_code(&app, "https://ok.com").await;
    let r = app
        .oneshot(
            Request::patch(format!("/admin/links/{code}"))
                .header("x-admin-token", "segredo")
                .header("content-type", "application/json")
                .body(Body::from(r#"{"url":"http://127.0.0.1:9000"}"#))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(r.status(), StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn admin_patch_url_invalida_400() {
    let app = app_admin("segredo").await;
    let code = cria_e_pega_code(&app, "https://ok.com").await;
    let r = app
        .oneshot(
            Request::patch(format!("/admin/links/{code}"))
                .header("x-admin-token", "segredo")
                .header("content-type", "application/json")
                .body(Body::from(r#"{"url":"ftp://nope"}"#))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(r.status(), StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn create_sem_token_quando_configurado_401() {
    let app = app_admin("segredo").await;
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
async fn create_com_token_quando_configurado_ok() {
    let app = app_admin("segredo").await;
    let resp = app
        .oneshot(
            Request::post("/")
                .header("content-type", "application/json")
                .header("x-admin-token", "segredo")
                .body(Body::from(r#"{"url":"https://example.com"}"#))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
}

#[tokio::test]
async fn cors_header_presente_quando_configurado() {
    // monta o router com uma origem permitida explícita (sem env)
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
    });
    let app = quark::api::router_with_cors(state, vec!["https://painel.example".into()]);
    let resp = app
        .oneshot(
            Request::get("/health")
                .header("origin", "https://painel.example")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(
        resp.headers().get("access-control-allow-origin").unwrap(),
        "https://painel.example"
    );
}
