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

/// Throwaway webhook dispatcher for the analytics worker: these tests exercise
/// pixel forwarding, not threshold alerts, so events are dropped.
fn noop_dispatcher() -> Arc<quark::webhooks::delivery::WebhookDispatcher> {
    let (wh_tx, _wh_rx) = tokio::sync::mpsc::channel(1);
    Arc::new(quark::webhooks::delivery::WebhookDispatcher::new(
        wh_tx,
        Arc::new(std::sync::atomic::AtomicBool::new(false)),
        Arc::new(std::sync::atomic::AtomicBool::new(false)),
    ))
}

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
        last_forward_at: None,
        last_forward_status: Default::default(),
    }
}

fn ev(id: u64, ts: u64) -> ClickEvent {
    ClickEvent {
        id,
        event_id: format!("clk_ev_{id}"),
        ts,
        referer: None,
        country: Some("BR".into()),
        user_agent: None,
        city: None,
        bot: false,
        ip: None,
        fbc: None,
        variant: None,
        tenant_id: 0,
    }
}

fn ev_t(id: u64, ts: u64, tenant: u64) -> ClickEvent {
    ClickEvent {
        tenant_id: tenant,
        ..ev(id, ts)
    }
}

#[tokio::test]
async fn worker_forwards_only_matching_tenant_events_to_each_pixel() {
    let (mock_base, captured) = mock_server("/mp/collect").await;
    let dir = tempfile::tempdir().unwrap();
    let (store, sink) = open_backends(dir.path(), true).await.unwrap();

    // Tenant A = DEFAULT_TENANT (0), semeado no boot. Tenant B = 1, criado agora.
    let tenant_b = quark::tenant::Tenant {
        id: quark::tenant::TenantId(1),
        name: "Tenant B".into(),
        slug: "tenant-b".into(),
        created: 0,
    };
    store.put_tenant(&tenant_b).await.unwrap();

    store
        .put_pixel(quark::tenant::DEFAULT_TENANT, &ga4_config(1))
        .await
        .unwrap();
    store
        .put_pixel(quark::tenant::TenantId(1), &ga4_config(1))
        .await
        .unwrap();

    let bases = PixelBases {
        ga4: mock_base,
        meta: "http://127.0.0.1:1".to_string(),
        anonymize_ip: false,
    };
    let (tx, rx) = tokio::sync::mpsc::channel::<ClickEvent>(100);
    let handle = spawn_worker(
        rx,
        sink.clone(),
        store.clone(),
        reqwest::Client::new(),
        KEY,
        bases,
        noop_dispatcher(),
        None,
    );

    tx.send(ev_t(10, 1_752_300_000, 0)).await.unwrap(); // tenant A
    tx.send(ev_t(20, 1_752_300_001, 1)).await.unwrap(); // tenant B
    drop(tx);
    handle.await.unwrap();

    let code_a = codec::to_base62(permute::encode(10, KEY));
    let code_b = codec::to_base62(permute::encode(20, KEY));

    let calls = captured.lock().unwrap();
    // Uma chamada por pixel (dois pixels), não uma só com o batch inteiro.
    assert_eq!(calls.len(), 2, "cada pixel encaminha uma vez");

    for (_, body) in calls.iter() {
        let events = body["events"].as_array().unwrap();
        let codes: Vec<&str> = events
            .iter()
            .map(|e| e["params"]["link_code"].as_str().unwrap())
            .collect();
        assert_eq!(codes.len(), 1, "cada chamada tem só o clique do seu tenant");
        assert!(
            codes[0] == code_a || codes[0] == code_b,
            "code inesperado: {:?}",
            codes[0]
        );
    }

    let seen: Vec<&str> = calls
        .iter()
        .map(|(_, b)| b["events"][0]["params"]["link_code"].as_str().unwrap())
        .collect();
    assert!(seen.contains(&code_a.as_str()), "tenant A não encaminhado");
    assert!(seen.contains(&code_b.as_str()), "tenant B não encaminhado");
}

#[tokio::test]
async fn worker_flush_forwards_batch_to_active_pixel_with_real_short_code() {
    let (mock_base, captured) = mock_server("/mp/collect").await;
    let dir = tempfile::tempdir().unwrap();
    let (store, sink) = open_backends(dir.path(), false).await.unwrap();
    store
        .put_pixel(quark::tenant::DEFAULT_TENANT, &ga4_config(1))
        .await
        .unwrap();

    let bases = PixelBases {
        ga4: mock_base,
        meta: "http://127.0.0.1:1".to_string(),
        anonymize_ip: false,
    };
    let (tx, rx) = tokio::sync::mpsc::channel::<ClickEvent>(100);
    let handle = spawn_worker(
        rx,
        sink.clone(),
        store.clone(),
        reqwest::Client::new(),
        KEY,
        bases,
        noop_dispatcher(),
        None,
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
    let (store, sink) = open_backends(dir.path(), false).await.unwrap();
    let mut inactive = ga4_config(1);
    inactive.active = false;
    store
        .put_pixel(quark::tenant::DEFAULT_TENANT, &inactive)
        .await
        .unwrap();

    let bases = PixelBases {
        ga4: mock_base,
        meta: "http://127.0.0.1:1".to_string(),
        anonymize_ip: false,
    };
    let (tx, rx) = tokio::sync::mpsc::channel::<ClickEvent>(100);
    let handle = spawn_worker(
        rx,
        sink.clone(),
        store.clone(),
        reqwest::Client::new(),
        KEY,
        bases,
        noop_dispatcher(),
        None,
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
    let (store, sink) = open_backends(dir.path(), false).await.unwrap();
    store
        .put_pixel(quark::tenant::DEFAULT_TENANT, &ga4_config(1))
        .await
        .unwrap();

    let bases = PixelBases {
        ga4: "http://127.0.0.1:1".to_string(),
        meta: "http://127.0.0.1:1".to_string(),
        anonymize_ip: false,
    };
    let (tx, rx) = tokio::sync::mpsc::channel::<ClickEvent>(100);
    let handle = spawn_worker(
        rx,
        sink.clone(),
        store.clone(),
        reqwest::Client::new(),
        KEY,
        bases,
        noop_dispatcher(),
        None,
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
    let (store, sink) = open_backends(dir.path(), false).await.unwrap();
    store
        .put_pixel(quark::tenant::DEFAULT_TENANT, &ga4_config(1))
        .await
        .unwrap();

    let bases = PixelBases {
        ga4: format!("http://{addr}"),
        meta: "http://127.0.0.1:1".to_string(),
        anonymize_ip: false,
    };
    let (tx, rx) = tokio::sync::mpsc::channel::<ClickEvent>(100);
    let handle = spawn_worker(
        rx,
        sink.clone(),
        store.clone(),
        reqwest::Client::new(),
        KEY,
        bases,
        noop_dispatcher(),
        None,
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
    let (store, sink) = open_backends(dir.path(), false).await.unwrap();
    // Deliberately no pixel configured before the worker starts: the
    // initial snapshot load must come back empty.

    let bases = PixelBases {
        ga4: mock_base,
        meta: "http://127.0.0.1:1".to_string(),
        anonymize_ip: false,
    };
    let (tx, rx) = tokio::sync::mpsc::channel::<ClickEvent>(100);
    let handle = spawn_worker(
        rx,
        sink.clone(),
        store.clone(),
        reqwest::Client::new(),
        KEY,
        bases,
        noop_dispatcher(),
        None,
    );

    // Added only now, after the worker's initial snapshot load already ran.
    store
        .put_pixel(quark::tenant::DEFAULT_TENANT, &ga4_config(1))
        .await
        .unwrap();

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

    let result = pixel::forward(&client, &closed_port_base, &config, &events, KEY, false).await;
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

/// A successful forward (mock 200) records passive pixel health as `Ok`,
/// with a `last_forward_at` timestamp (LUC-87 fase 3).
#[tokio::test]
async fn worker_flush_records_pixel_health_ok_on_successful_forward() {
    let (mock_base, _captured) = mock_server("/mp/collect").await;
    let dir = tempfile::tempdir().unwrap();
    let (store, sink) = open_backends(dir.path(), false).await.unwrap();
    store
        .put_pixel(quark::tenant::DEFAULT_TENANT, &ga4_config(1))
        .await
        .unwrap();

    let bases = PixelBases {
        ga4: mock_base,
        meta: "http://127.0.0.1:1".to_string(),
        anonymize_ip: false,
    };
    let (tx, rx) = tokio::sync::mpsc::channel::<ClickEvent>(100);
    let handle = spawn_worker(
        rx,
        sink.clone(),
        store.clone(),
        reqwest::Client::new(),
        KEY,
        bases,
        noop_dispatcher(),
        None,
    );

    tx.send(ev(42, 1_752_300_000)).await.unwrap();
    drop(tx);
    handle.await.unwrap();

    let pixel = store
        .get_pixel(quark::tenant::DEFAULT_TENANT, 1)
        .await
        .unwrap()
        .expect("pixel still exists");
    assert_eq!(pixel.last_forward_status, quark::health::HealthStatus::Ok);
    assert!(pixel.last_forward_at.is_some());
}

/// A failed forward (mock 500) records passive pixel health as `Error`,
/// still with a `last_forward_at` timestamp (LUC-87 fase 3).
#[tokio::test]
async fn worker_flush_records_pixel_health_error_on_failed_forward() {
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
    let (store, sink) = open_backends(dir.path(), false).await.unwrap();
    store
        .put_pixel(quark::tenant::DEFAULT_TENANT, &ga4_config(1))
        .await
        .unwrap();

    let bases = PixelBases {
        ga4: format!("http://{addr}"),
        meta: "http://127.0.0.1:1".to_string(),
        anonymize_ip: false,
    };
    let (tx, rx) = tokio::sync::mpsc::channel::<ClickEvent>(100);
    let handle = spawn_worker(
        rx,
        sink.clone(),
        store.clone(),
        reqwest::Client::new(),
        KEY,
        bases,
        noop_dispatcher(),
        None,
    );

    tx.send(ev(9, 1_752_300_000)).await.unwrap();
    drop(tx);
    handle.await.unwrap();

    let pixel = store
        .get_pixel(quark::tenant::DEFAULT_TENANT, 1)
        .await
        .unwrap()
        .expect("pixel still exists");
    assert!(matches!(
        pixel.last_forward_status,
        quark::health::HealthStatus::Error(_)
    ));
    assert!(pixel.last_forward_at.is_some());
}
