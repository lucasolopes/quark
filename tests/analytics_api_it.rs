use axum::body::Body;
use axum::http::{Request, StatusCode};
use quark::api::{router, AppState};
use quark::cache::Cache;
use quark::store::open_backends;
use std::sync::Arc;
use tower::ServiceExt;

fn app_with(
    admin: Option<&str>,
    chan_cap: usize,
) -> (
    axum::Router,
    tokio::sync::mpsc::Receiver<quark::analytics::ClickEvent>,
) {
    let dir = Box::leak(Box::new(tempfile::tempdir().unwrap()));
    let (store, sink) = open_backends(dir.path()).unwrap();
    let cache = Cache::new(store.clone(), 1000);
    let (tx, rx) = tokio::sync::mpsc::channel(chan_cap);
    let state = Arc::new(AppState {
        cache,
        store,
        key: 0x1234,
        analytics_tx: tx,
        sink,
        admin_token: admin.map(|s| s.to_string()),
    });
    (router(state), rx)
}

async fn create(app: &axum::Router, url: &str) -> String {
    let resp = app
        .clone()
        .oneshot(
            Request::post("/")
                .header("content-type", "application/json")
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
async fn redirect_nao_bloqueia_com_fila_cheia() {
    // canal capacidade 1, SEM worker consumindo: enche na 1ª e descarta o resto,
    // mas o redirect precisa continuar respondendo 302.
    let (app, _rx) = app_with(None, 1);
    let code = create(&app, "https://example.com").await;
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
        assert_eq!(resp.status(), StatusCode::FOUND); // 302 sempre, mesmo com fila cheia
    }
}

#[tokio::test]
async fn stats_exige_token() {
    let (app, _rx) = app_with(Some("segredo"), 100);
    let code = create(&app, "https://example.com").await;
    // sem token → 401
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
    // com token errado → 401
    let resp = app
        .clone()
        .oneshot(
            Request::get(format!("/{code}/stats"))
                .header("x-admin-token", "errado")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
    // com token certo → 200, shape consistente (aggregates é objeto, não null)
    let resp = app
        .clone()
        .oneshot(
            Request::get(format!("/{code}/stats"))
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
    assert!(v["aggregates"].is_object());
    assert!(v["recent"].is_array());
}

#[tokio::test]
async fn stats_404_codigo_inexistente() {
    let (app, _rx) = app_with(Some("segredo"), 100);
    // "0000000" decodifica p/ id 0, in-range, nunca criado neste store fresco.
    let resp = app
        .clone()
        .oneshot(
            Request::get("/0000000/stats")
                .header("x-admin-token", "segredo")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn stats_desligado_sem_token_configurado() {
    let (app, _rx) = app_with(None, 100); // admin_token None
    let code = create(&app, "https://example.com").await;
    let resp = app
        .clone()
        .oneshot(
            Request::get(format!("/{code}/stats"))
                .header("x-admin-token", "qualquer")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
}
