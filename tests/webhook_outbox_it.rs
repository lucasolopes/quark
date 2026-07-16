use axum::extract::State;
use axum::http::HeaderMap;
use axum::routing::any;
use axum::Router;
use quark::store::{postgres::PostgresStore, OutboxRow, Store};
use quark::webhooks::delivery::{poll_once, MAX_DELIVERY_ATTEMPTS, RELAY_BATCH};
use quark::webhooks::{EventType, SubscriptionKind, WebhookSubscription};
use serial_test::serial;
use sqlx::postgres::PgPoolOptions;
use sqlx::{PgPool, Row};
use std::collections::HashMap;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::net::TcpListener;

/// A test secret from the Standard Webhooks reference vectors.
const TEST_SECRET: &str = "whsec_MfKQ9r8GKYqrTwjUPD8ILPZIo2LaLaSw";

/// Opens a fresh store (schema created, all tables truncated) plus a raw pool
/// used to inspect `webhook_deliveries` rows the `Store` trait does not expose.
/// Returns `None` when `QUARK_TEST_DATABASE_URL` is unset so the gated tests
/// skip cleanly.
async fn setup() -> Option<(Arc<dyn Store>, PgPool)> {
    let url = std::env::var("QUARK_TEST_DATABASE_URL").ok()?;
    let s = PostgresStore::open(&url, false).await.unwrap();
    s.reset_for_tests().await.unwrap();
    let pool = PgPoolOptions::new()
        .max_connections(6)
        .connect(&url)
        .await
        .unwrap();
    let store: Arc<dyn Store> = Arc::new(s);
    Some((store, pool))
}

#[derive(Debug, Clone)]
struct Captured {
    headers: HashMap<String, String>,
}

struct ServerState {
    captured: Mutex<Vec<Captured>>,
    responses: Vec<u16>,
    next: AtomicUsize,
}

async fn handler(
    State(state): State<Arc<ServerState>>,
    headers: HeaderMap,
) -> axum::http::StatusCode {
    let mut map = HashMap::new();
    for (k, v) in headers.iter() {
        map.insert(
            k.as_str().to_ascii_lowercase(),
            v.to_str().unwrap().to_string(),
        );
    }
    state
        .captured
        .lock()
        .unwrap()
        .push(Captured { headers: map });
    let idx = state.next.fetch_add(1, Ordering::SeqCst);
    let code = state
        .responses
        .get(idx)
        .copied()
        .unwrap_or(*state.responses.last().unwrap());
    axum::http::StatusCode::from_u16(code).unwrap()
}

/// Spins a local mock server replying with `responses` in sequence (repeating
/// the last entry). Returns its base URL and the shared capture state.
async fn spawn_mock(responses: Vec<u16>) -> (String, Arc<ServerState>) {
    let state = Arc::new(ServerState {
        captured: Mutex::new(Vec::new()),
        responses,
        next: AtomicUsize::new(0),
    });
    let app = Router::new()
        .route("/hook", any(handler))
        .with_state(state.clone());
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    (format!("http://{addr}/hook"), state)
}

fn relay_client() -> reqwest::Client {
    reqwest::Client::builder()
        .timeout(Duration::from_secs(5))
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .unwrap()
}

async fn add_sub(store: &Arc<dyn Store>, url: &str) -> WebhookSubscription {
    let id = store
        .next_webhook_id(quark::tenant::DEFAULT_TENANT)
        .await
        .unwrap();
    let sub = WebhookSubscription {
        id,
        url: url.to_string(),
        events: vec![EventType::LinkCreated],
        secret: TEST_SECRET.to_string(),
        active: true,
        created: 1,
        kind: SubscriptionKind::Generic,
    };
    store
        .put_webhook(quark::tenant::DEFAULT_TENANT, &sub)
        .await
        .unwrap();
    sub
}

fn payload(event_id: &str) -> String {
    serde_json::json!({
        "id": event_id,
        "type": "link.created",
        "timestamp": 1,
        "data": {"code": "abc123", "url": "https://e.com"},
    })
    .to_string()
}

fn row(delivery_key: &str, sub_id: u64, at: u64) -> OutboxRow {
    OutboxRow {
        delivery_key: delivery_key.to_string(),
        subscription_id: sub_id,
        event_type: "link.created".to_string(),
        payload: payload("evt_test"),
        created: at,
        next_attempt_at: at,
        tenant_id: quark::tenant::DEFAULT_TENANT,
    }
}

/// Full row state for assertions.
async fn row_state(pool: &PgPool, key: &str) -> (i32, i64, Option<i64>, bool) {
    let r = sqlx::query(
        "SELECT attempts, next_attempt_at, delivered_at, dead FROM webhook_deliveries WHERE delivery_key=$1",
    )
    .bind(key)
    .fetch_one(pool)
    .await
    .unwrap();
    (
        r.try_get("attempts").unwrap(),
        r.try_get("next_attempt_at").unwrap(),
        r.try_get("delivered_at").unwrap(),
        r.try_get("dead").unwrap(),
    )
}

async fn count_rows(pool: &PgPool, key: &str) -> i64 {
    let r = sqlx::query("SELECT COUNT(*) AS n FROM webhook_deliveries WHERE delivery_key=$1")
        .bind(key)
        .fetch_one(pool)
        .await
        .unwrap();
    r.try_get("n").unwrap()
}

/// Enqueue -> relay delivers to the mock server -> every row marked delivered.
#[tokio::test]
#[serial(pg)]
async fn relay_delivers_all_enqueued() {
    let Some((store, pool)) = setup().await else {
        eprintln!("skip: QUARK_TEST_DATABASE_URL not set");
        return;
    };
    let (url, mock) = spawn_mock(vec![200]).await;
    let a = add_sub(&store, &url).await;
    let b = add_sub(&store, &url).await;
    let now = quark::now();
    let key_a = format!("evt_test.{}", a.id);
    let key_b = format!("evt_test.{}", b.id);
    store
        .enqueue_deliveries(&[row(&key_a, a.id, now), row(&key_b, b.id, now)])
        .await
        .unwrap();

    let subs = store
        .list_webhooks(quark::tenant::DEFAULT_TENANT)
        .await
        .unwrap();
    let claimed = poll_once(&store, &relay_client(), &subs, now, RELAY_BATCH, |_| false).await;
    assert_eq!(claimed, 2);

    assert_eq!(mock.captured.lock().unwrap().len(), 2);
    assert!(row_state(&pool, &key_a).await.2.is_some());
    assert!(row_state(&pool, &key_b).await.2.is_some());
}

/// Regression (found live): a delivery whose subscription is NOT in the relay's
/// cached snapshot (e.g. the sub was created after the last 10s refresh) must
/// NOT be dead-lettered. The relay looks the sub up in the store authoritatively
/// and delivers it. Before the fix it was marked dead with attempts=0.
#[tokio::test]
#[serial(pg)]
async fn delivery_for_sub_missing_from_snapshot_is_looked_up_not_dead_lettered() {
    let Some((store, pool)) = setup().await else {
        eprintln!("skip: QUARK_TEST_DATABASE_URL not set");
        return;
    };
    let (url, mock) = spawn_mock(vec![200]).await;
    let sub = add_sub(&store, &url).await;
    let now = quark::now();
    let key = format!("evt_test.{}", sub.id);
    store
        .enqueue_deliveries(&[row(&key, sub.id, now)])
        .await
        .unwrap();

    let claimed = poll_once(&store, &relay_client(), &[], now, RELAY_BATCH, |_| false).await;
    assert_eq!(claimed, 1);

    assert_eq!(mock.captured.lock().unwrap().len(), 1);
    let (_attempts, _next, delivered_at, dead) = row_state(&pool, &key).await;
    assert!(
        delivered_at.is_some(),
        "delivered via store lookup, not dropped"
    );
    assert!(!dead, "must not be dead-lettered on a snapshot miss");
}

/// A 500 endpoint: attempts grow, next_attempt_at grows, then dead=true after
/// MAX (DLQ), and the row stops being claimed.
#[tokio::test]
#[serial(pg)]
async fn failing_endpoint_dead_letters_after_max() {
    let Some((store, pool)) = setup().await else {
        return;
    };
    let (url, _mock) = spawn_mock(vec![500]).await;
    let sub = add_sub(&store, &url).await;
    let key = format!("evt_test.{}", sub.id);
    let base = quark::now();
    store
        .enqueue_deliveries(&[row(&key, sub.id, base)])
        .await
        .unwrap();
    let subs = store
        .list_webhooks(quark::tenant::DEFAULT_TENANT)
        .await
        .unwrap();
    let client = relay_client();

    let mut last_next = 0i64;
    let mut prev_attempts = 0i32;
    for i in 0..MAX_DELIVERY_ATTEMPTS {
        let now = base + (i as u64) * 100_000;
        let claimed = poll_once(&store, &client, &subs, now, RELAY_BATCH, |_| false).await;
        assert_eq!(claimed, 1, "row must be claimable on attempt {i}");
        let (attempts, next_attempt_at, delivered_at, dead) = row_state(&pool, &key).await;
        assert!(delivered_at.is_none());
        if !dead {
            assert!(attempts > prev_attempts, "attempts must grow");
            assert!(
                next_attempt_at > last_next,
                "next_attempt_at must grow ({next_attempt_at} > {last_next})"
            );
            prev_attempts = attempts;
            last_next = next_attempt_at;
        } else {
            break;
        }
    }

    let (attempts, _next, delivered_at, dead) = row_state(&pool, &key).await;
    assert!(dead, "row must be dead-lettered after MAX attempts");
    assert!(delivered_at.is_none());
    assert_eq!(attempts as u32, MAX_DELIVERY_ATTEMPTS);

    let far = base + 100_000_000;
    let claimed = poll_once(&store, &client, &subs, far, RELAY_BATCH, |_| false).await;
    assert_eq!(claimed, 0, "a dead row must not be claimed again");
}

/// Two concurrent claims return disjoint rows (SKIP LOCKED, no double delivery).
#[tokio::test]
#[serial(pg)]
async fn concurrent_claims_are_disjoint() {
    let Some((store, _pool)) = setup().await else {
        return;
    };
    let sub = add_sub(&store, "https://e.com/hook").await;
    let now = quark::now();
    let rows: Vec<OutboxRow> = (0..20)
        .map(|i| row(&format!("evt_{i}.{}", sub.id), sub.id, now))
        .collect();
    store.enqueue_deliveries(&rows).await.unwrap();

    let s1 = store.clone();
    let s2 = store.clone();
    let (a, b) = tokio::join!(
        async move { s1.claim_due_deliveries(now, 20).await.unwrap() },
        async move { s2.claim_due_deliveries(now, 20).await.unwrap() },
    );

    let ids_a: std::collections::HashSet<i64> = a.iter().map(|d| d.id).collect();
    let ids_b: std::collections::HashSet<i64> = b.iter().map(|d| d.id).collect();
    assert!(
        ids_a.is_disjoint(&ids_b),
        "concurrent claims overlapped: {ids_a:?} vs {ids_b:?}"
    );
    assert_eq!(a.len() + b.len(), 20, "every row claimed exactly once");
}

/// The webhook-id header equals delivery_key and is identical across two
/// attempts of the same row (the persisted idempotency key).
#[tokio::test]
#[serial(pg)]
async fn webhook_id_equals_delivery_key_across_attempts() {
    let Some((store, _pool)) = setup().await else {
        return;
    };
    let (url, mock) = spawn_mock(vec![500, 200]).await;
    let sub = add_sub(&store, &url).await;
    let key = format!("evt_test.{}", sub.id);
    let base = quark::now();
    store
        .enqueue_deliveries(&[row(&key, sub.id, base)])
        .await
        .unwrap();
    let subs = store
        .list_webhooks(quark::tenant::DEFAULT_TENANT)
        .await
        .unwrap();
    let client = relay_client();

    poll_once(&store, &client, &subs, base, RELAY_BATCH, |_| false).await;
    poll_once(
        &store,
        &client,
        &subs,
        base + 100_000_000,
        RELAY_BATCH,
        |_| false,
    )
    .await;

    let captured = mock.captured.lock().unwrap();
    assert_eq!(captured.len(), 2, "row should be attempted twice");
    for req in captured.iter() {
        assert_eq!(
            req.headers.get("webhook-id").map(String::as_str),
            Some(key.as_str())
        );
    }
    assert_eq!(
        captured[0].headers.get("webhook-id"),
        captured[1].headers.get("webhook-id")
    );
}

/// ON CONFLICT (delivery_key) DO NOTHING: enqueuing the same (event, sub) twice
/// inserts a single row.
#[tokio::test]
#[serial(pg)]
async fn duplicate_enqueue_inserts_one_row() {
    let Some((store, pool)) = setup().await else {
        return;
    };
    let sub = add_sub(&store, "https://e.com/hook").await;
    let key = format!("evt_test.{}", sub.id);
    let now = quark::now();
    store
        .enqueue_deliveries(&[row(&key, sub.id, now)])
        .await
        .unwrap();
    store
        .enqueue_deliveries(&[row(&key, sub.id, now)])
        .await
        .unwrap();
    assert_eq!(count_rows(&pool, &key).await, 1);
}

/// `claim_due_deliveries` must hand back the `tenant_id` a row was enqueued
/// with (not always `DEFAULT_TENANT`): the relay's `deliver_claimed` resolves
/// the subscription via `get_webhook(delivery.tenant_id, ...)`, so a wrong or
/// dropped `tenant_id` here would make the relay look up the subscription in
/// the wrong tenant once P2b creates real tenants.
#[tokio::test]
#[serial(pg)]
async fn claim_due_deliveries_round_trips_tenant_id() {
    let Some((store, _pool)) = setup().await else {
        eprintln!("skip: QUARK_TEST_DATABASE_URL not set");
        return;
    };
    let tenant = quark::tenant::TenantId(5);
    let now = quark::now();
    let mut r = row("evt_tenant_test.1", 1, now);
    r.tenant_id = tenant;
    store.enqueue_deliveries(&[r]).await.unwrap();

    let claimed = store.claim_due_deliveries(now, 10).await.unwrap();
    let d = claimed
        .iter()
        .find(|d| d.delivery_key == "evt_tenant_test.1")
        .expect("the enqueued row must be claimable");
    assert_eq!(d.tenant_id, tenant);
}
