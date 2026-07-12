use axum::body::Body;
use axum::http::{Request, StatusCode};
use quark::api::{router, AppState};
use quark::cache::Cache;
use quark::store::Store;
use std::sync::Arc;
use tower::ServiceExt; // oneshot

fn app() -> axum::Router {
    let dir = Box::leak(Box::new(tempfile::tempdir().unwrap()));
    let store = Arc::new(Store::open(dir.path()).unwrap());
    let cache = Cache::new(store.clone(), 1000);
    let state = Arc::new(AppState { cache, store, key: 0x1234 });
    router(state)
}

#[tokio::test]
async fn cria_e_redireciona() {
    let app = app();
    let resp = app.clone().oneshot(
        Request::post("/").header("content-type", "application/json")
            .body(Body::from(r#"{"url":"https://example.com"}"#)).unwrap()
    ).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX).await.unwrap();
    let v: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    let code = v["code"].as_str().unwrap().to_string();

    let resp = app.oneshot(Request::get(format!("/{code}")).body(Body::empty()).unwrap())
        .await.unwrap();
    assert_eq!(resp.status(), StatusCode::FOUND); // 302
    assert_eq!(resp.headers()["location"], "https://example.com");
}

#[tokio::test]
async fn codigo_inexistente_404() {
    let app = app();
    let resp = app.oneshot(Request::get("/0000000").body(Body::empty()).unwrap())
        .await.unwrap();
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
}
