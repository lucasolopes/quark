use axum::extract::State;
use axum::routing::post;
use axum::Router;
use quark::analytics::{spawn_worker, ClickEvent};
use quark::codec;
use quark::permute;
use quark::pixel::{self, PixelBases, PixelConfig, PixelCredentials, Provider};
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

/// The worker caches the pixel-config list and only refreshes it on the
/// ticker (never on the flush path itself, see the review that produced
/// this test: a store call per flush can stall the worker on a wedged
/// store). A pixel added *after* the worker has started is invisible until
/// the next tick refreshes the snapshot; once it does, forwarding uses the
/// refreshed snapshot with no further store call needed on the flush path.
#[tokio::test]
async fn worker_forwards_to_a_pixel_added_after_start_once_the_snapshot_refreshes() {
    let (mock_base, captured) = mock_server("/mp/collect").await;
    let dir = tempfile::tempdir().unwrap();
    let (store, sink) = open_backends(dir.path()).await.unwrap();
    // Deliberately no pixel configured before the worker starts: the
    // initial snapshot load must come back empty.

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

    // Added only now, after the worker's initial snapshot load already ran.
    store.put_pixel(&ga4_config(1)).await.unwrap();

    // Past the 5s ticker: the worker refreshes its cached snapshot here,
    // with no event flowing through the channel (so no per-flush store
    // call is exercised, only the ticker-driven refresh).
    tokio::time::sleep(std::time::Duration::from_millis(5_500)).await;

    tx.send(ev(42, 1_752_300_000)).await.unwrap();
    drop(tx);
    handle.await.unwrap();

    let calls = captured.lock().unwrap();
    assert_eq!(
        calls.len(),
        1,
        "pixel added after worker start should be forwarded once the cached snapshot refreshes"
    );
}

/// Security regression: a forward failure must never leak the provider URL
/// (and therefore the credential embedded in its query string) through the
/// `PixelError` that callers log via `Display`/`to_string()`. Points a
/// config carrying a recognizable credential sentinel at a closed local
/// port so the send fails, then asserts the resulting error's string form
/// contains neither the sentinel nor a query string.
#[tokio::test]
async fn forward_error_display_never_contains_provider_url_or_credentials() {
    let mut config = ga4_config(1);
    config.credentials.api_secret = Some("SECRETSENTINEL".into());

    let closed_port_base = "http://127.0.0.1:1".to_string();
    let events = vec![ev(1, 1_752_300_000)];
    let client = reqwest::Client::new();

    let result = pixel::forward(&client, &closed_port_base, &config, &events, KEY).await;
    let err = result.expect_err("connection to a closed port must fail");
    let message = err.to_string();

    assert!(
        !message.contains("SECRETSENTINEL"),
        "error message leaked the credential: {message}"
    );
    assert!(
        !message.contains("api_secret"),
        "error message leaked the query string: {message}"
    );
    assert!(
        !message.contains("measurement_id"),
        "error message leaked the provider URL: {message}"
    );
    assert!(
        !message.contains('?'),
        "error message still embeds a query string: {message}"
    );
}
