use axum::extract::State;
use axum::routing::post;
use axum::Router;
use quark::analytics::{spawn_worker, ClickEvent};
use quark::codec;
use quark::permute;
use quark::pixel::{PixelBases, PixelConfig, PixelCredentials, Provider};
use quark::store::open_backends;
use serde_json::Value;
use std::sync::{Arc, Mutex};
use tokio::net::TcpListener;

const KEY: u64 = 0x1234;

type Captured = Arc<Mutex<Vec<(String, Value)>>>;

/// A tiny mock provider: always 200s and records every POST it receives.
async fn mock_server(path: &'static str) -> (String, Captured) {
    let captured: Captured = Arc::new(Mutex::new(Vec::new()));
    let state = captured.clone();

    async fn handler(
        State(state): State<Captured>,
        req: axum::extract::Request,
    ) -> axum::http::StatusCode {
        let (parts, body) = req.into_parts();
        let full_path = parts
            .uri
            .path_and_query()
            .map(|pq| pq.as_str().to_string())
            .unwrap_or_default();
        let bytes = axum::body::to_bytes(body, usize::MAX).await.unwrap();
        let json: Value = serde_json::from_slice(&bytes).unwrap();
        state.lock().unwrap().push((full_path, json));
        axum::http::StatusCode::OK
    }

    let app = Router::new().route(path, post(handler)).with_state(state);
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    (format!("http://{addr}"), captured)
}

fn ga4_config(id: u64) -> PixelConfig {
    PixelConfig {
        id,
        provider: Provider::Ga4,
        credentials: PixelCredentials {
            measurement_id: Some("G-ABC123".into()),
            api_secret: Some("secret1".into()),
            pixel_id: None,
            access_token: None,
        },
        active: true,
        created: 0,
    }
}

fn ev(id: u64, ts: u64) -> ClickEvent {
    ClickEvent {
        id,
        ts,
        referer: None,
        country: Some("BR".into()),
        user_agent: None,
    }
}

#[tokio::test]
async fn worker_flush_forwards_batch_to_active_pixel_with_real_short_code() {
    let (mock_base, captured) = mock_server("/mp/collect").await;
    let dir = tempfile::tempdir().unwrap();
    let (store, sink) = open_backends(dir.path()).await.unwrap();
    store.put_pixel(&ga4_config(1)).await.unwrap();

    let bases = PixelBases {
        ga4: mock_base,
        meta: "http://127.0.0.1:1".to_string(),
    };
    let (tx, rx) = tokio::sync::mpsc::channel::<ClickEvent>(100);
    let handle = spawn_worker(
        rx,
        sink.clone(),
        store.clone(),
        reqwest::Client::new(),
        KEY,
        bases,
    );

    tx.send(ev(42, 1_752_300_000)).await.unwrap();
    drop(tx);
    handle.await.unwrap();

    let s = sink.stats(42).await.unwrap().unwrap();
    assert_eq!(s.aggregates.total, 1);

    let calls = captured.lock().unwrap();
    assert_eq!(calls.len(), 1);
    let (path, body) = &calls[0];
    assert!(path.starts_with("/mp/collect?measurement_id=G-ABC123"));
    let expected_code = codec::to_base62(permute::encode(42, KEY));
    assert_ne!(expected_code, "42");
    assert_eq!(body["events"][0]["params"]["link_code"], expected_code);
}

#[tokio::test]
async fn worker_flush_skips_inactive_pixel_configs() {
    let (mock_base, captured) = mock_server("/mp/collect").await;
    let dir = tempfile::tempdir().unwrap();
    let (store, sink) = open_backends(dir.path()).await.unwrap();
    let mut inactive = ga4_config(1);
    inactive.active = false;
    store.put_pixel(&inactive).await.unwrap();

    let bases = PixelBases {
        ga4: mock_base,
        meta: "http://127.0.0.1:1".to_string(),
    };
    let (tx, rx) = tokio::sync::mpsc::channel::<ClickEvent>(100);
    let handle = spawn_worker(
        rx,
        sink.clone(),
        store.clone(),
        reqwest::Client::new(),
        KEY,
        bases,
    );

    tx.send(ev(1, 1_752_300_000)).await.unwrap();
    drop(tx);
    handle.await.unwrap();

    assert!(captured.lock().unwrap().is_empty());
}

/// The core fail-open guarantee: a down/erroring provider must never break
/// the worker or keep the sink from recording the batch.
#[tokio::test]
async fn worker_flush_is_fail_open_when_provider_is_down() {
    let dir = tempfile::tempdir().unwrap();
    let (store, sink) = open_backends(dir.path()).await.unwrap();
    store.put_pixel(&ga4_config(1)).await.unwrap();

    let bases = PixelBases {
        ga4: "http://127.0.0.1:1".to_string(),
        meta: "http://127.0.0.1:1".to_string(),
    };
    let (tx, rx) = tokio::sync::mpsc::channel::<ClickEvent>(100);
    let handle = spawn_worker(
        rx,
        sink.clone(),
        store.clone(),
        reqwest::Client::new(),
        KEY,
        bases,
    );

    tx.send(ev(7, 1_752_300_000)).await.unwrap();
    drop(tx);
    handle.await.unwrap();

    let s = sink.stats(7).await.unwrap().unwrap();
    assert_eq!(s.aggregates.total, 1);
}

/// Same fail-open guarantee, but the provider is reachable and responds
/// with a server error rather than refusing the connection.
#[tokio::test]
async fn worker_flush_is_fail_open_when_provider_returns_500() {
    async fn err_handler() -> axum::http::StatusCode {
        axum::http::StatusCode::INTERNAL_SERVER_ERROR
    }
    let app = Router::new().route("/mp/collect", post(err_handler));
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });

    let dir = tempfile::tempdir().unwrap();
    let (store, sink) = open_backends(dir.path()).await.unwrap();
    store.put_pixel(&ga4_config(1)).await.unwrap();

    let bases = PixelBases {
        ga4: format!("http://{addr}"),
        meta: "http://127.0.0.1:1".to_string(),
    };
    let (tx, rx) = tokio::sync::mpsc::channel::<ClickEvent>(100);
    let handle = spawn_worker(
        rx,
        sink.clone(),
        store.clone(),
        reqwest::Client::new(),
        KEY,
        bases,
    );

    tx.send(ev(9, 1_752_300_000)).await.unwrap();
    drop(tx);
    handle.await.unwrap();

    let s = sink.stats(9).await.unwrap().unwrap();
    assert_eq!(s.aggregates.total, 1);
}
