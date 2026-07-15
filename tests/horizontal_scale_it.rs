//! Horizontal scale: proves that replicas over the same Postgres generate unique
//! IDs and share data. Gated by QUARK_TEST_DATABASE_URL; without the env var,
//! the tests skip (but always compile).

use axum::body::Body;
use axum::http::{Request, StatusCode};
use quark::analytics::AnalyticsSink;
use quark::api::{router, AppState};
use quark::cache::Cache;
use quark::store::{postgres::PostgresStore, Store};
use serial_test::serial;
use std::collections::HashSet;
use std::sync::Arc;
use tower::ServiceExt;

fn test_url() -> Option<String> {
    std::env::var("QUARK_TEST_DATABASE_URL").ok()
}

/// Builds a complete quark router over an already-open Postgres — simulates a replica.
async fn pg_replica(url: &str) -> axum::Router {
    let pg = Arc::new(PostgresStore::open(url).await.unwrap());
    let store: Arc<dyn Store> = pg.clone();
    let sink: Arc<dyn AnalyticsSink> = pg;
    let cache = Cache::new(store.clone(), 1000, None);
    let (analytics_tx, _rx) = tokio::sync::mpsc::channel(100);
    let state = Arc::new(AppState {
        oidc: None,
        oidc_configured: false,
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
#[serial(pg)]
async fn unique_ids_across_replicas_pg() {
    let Some(url) = test_url() else {
        eprintln!("skip: QUARK_TEST_DATABASE_URL not set");
        return;
    };
    PostgresStore::open(&url)
        .await
        .unwrap()
        .reset_for_tests()
        .await
        .unwrap();

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
        assert!(ids.insert(id), "duplicate id across replicas: {id}");
    }
    assert_eq!(ids.len(), 400, "expected 400 unique ids");
}

#[tokio::test]
#[serial(pg)]
async fn create_on_replica_a_redirect_on_replica_b_pg() {
    let Some(url) = test_url() else {
        eprintln!("skip: QUARK_TEST_DATABASE_URL not set");
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

    let resp = app_a
        .oneshot(
            Request::post("/")
                .header("content-type", "application/json")
                .body(Body::from(r#"{"url":"https://example.com/replica"}"#))
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

    let resp = app_b
        .oneshot(
            Request::get(format!("/{code}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::FOUND);
    assert_eq!(resp.headers()["location"], "https://example.com/replica");
}
