//! Escala horizontal: prova que réplicas sobre o mesmo Postgres geram IDs
//! únicos e compartilham dados. Gated por QUARK_TEST_DATABASE_URL; sem a env,
//! os testes pulam (mas compilam sempre).

use axum::body::Body;
use axum::http::{Request, StatusCode};
use quark::analytics::AnalyticsSink;
use quark::api::{router, AppState};
use quark::cache::Cache;
use quark::store::{postgres::PostgresStore, Store};
use serial_test::serial;
use std::collections::HashSet;
use std::sync::Arc;
use tower::ServiceExt; // oneshot

fn test_url() -> Option<String> {
    std::env::var("QUARK_TEST_DATABASE_URL").ok()
}

/// Monta um router quark completo sobre um Postgres já aberto — simula uma réplica.
async fn pg_replica(url: &str) -> axum::Router {
    let pg = Arc::new(PostgresStore::open(url).await.unwrap());
    let store: Arc<dyn Store> = pg.clone();
    let sink: Arc<dyn AnalyticsSink> = pg;
    let cache = Cache::new(store.clone(), 1000);
    let (analytics_tx, _rx) = tokio::sync::mpsc::channel(100);
    let state = Arc::new(AppState {
        cache,
        store,
        key: 0x1234, // mesma key em todas as réplicas (como em produção)
        analytics_tx,
        sink,
        admin_token: None,
    });
    router(state)
}

#[tokio::test]
#[serial(pg)]
async fn ids_unicos_entre_replicas_pg() {
    let Some(url) = test_url() else {
        eprintln!("skip: sem QUARK_TEST_DATABASE_URL");
        return;
    };
    // limpa o estado uma vez
    PostgresStore::open(&url)
        .await
        .unwrap()
        .reset_for_tests()
        .await
        .unwrap();

    // duas "réplicas" = dois stores independentes sobre o mesmo banco
    let a = Arc::new(PostgresStore::open(&url).await.unwrap());
    let b = Arc::new(PostgresStore::open(&url).await.unwrap());

    let mut handles = Vec::new();
    for store in [a.clone(), b.clone()] {
        for _ in 0..200 {
            let st = store.clone();
            handles.push(tokio::spawn(async move { st.next_id().await.unwrap() }));
        }
    }

    let mut ids = HashSet::new();
    for h in handles {
        let id = h.await.unwrap();
        assert!(ids.insert(id), "id duplicado entre réplicas: {id}");
    }
    assert_eq!(ids.len(), 400, "esperava 400 ids únicos");
}

#[tokio::test]
#[serial(pg)]
async fn create_na_replica_a_redirect_na_replica_b_pg() {
    let Some(url) = test_url() else {
        eprintln!("skip: sem QUARK_TEST_DATABASE_URL");
        return;
    };
    PostgresStore::open(&url)
        .await
        .unwrap()
        .reset_for_tests()
        .await
        .unwrap();

    let app_a = pg_replica(&url).await;
    let app_b = pg_replica(&url).await;

    // cria o link na réplica A
    let resp = app_a
        .oneshot(
            Request::post("/")
                .header("content-type", "application/json")
                .body(Body::from(r#"{"url":"https://exemplo.com/replica"}"#))
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

    // resolve o mesmo código na réplica B (cache frio em B → busca no store compartilhado)
    let resp = app_b
        .oneshot(
            Request::get(format!("/{code}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::FOUND);
    assert_eq!(resp.headers()["location"], "https://exemplo.com/replica");
}
