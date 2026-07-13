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
    let state = Arc::new(AppState {
        cache,
        store,
        key: 0x1234,
    });
    router(state)
}

#[tokio::test]
async fn cria_e_redireciona() {
    let app = app();
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
    let app = app();
    let resp = app
        .oneshot(Request::get("/0000000").body(Body::empty()).unwrap())
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn alias_em_uso_409() {
    let app = app();
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
    let app = app();
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
    let app = app();
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
    let app = app();
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
async fn redirect_sem_ttl_tem_cache_control_default() {
    let app = app();
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
    let app = app();
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
    let app = app();
    let resp = app
        .oneshot(Request::get("/0000000").body(Body::empty()).unwrap())
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
    assert_eq!(resp.headers()["cache-control"], "no-store");
}

#[tokio::test]
async fn ttl_overflow_400() {
    let app = app();
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
