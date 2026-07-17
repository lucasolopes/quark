use quark::analytics::AnalyticsSink;
use quark::api::AppState;
use quark::cache::Cache;
use quark::invalidate::{spawn_invalidation_subscriber, Invalidator};
use quark::store::{open_backends, Record, Store};
use std::sync::atomic::AtomicBool;
use std::sync::Arc;
use std::time::{Duration, Instant};

/// These tests need a live Valkey/Redis. They are skipped (returning early)
/// unless `QUARK_TEST_VALKEY_URL` is set, mirroring `tests/valkey_tier_it.rs`.
/// They cover the cross-node path end to end: node A's request-path
/// `invalidate` publishes on `quark:invalidate`, node B's dedicated subscriber
/// receives it and drops B's stale L1.
fn rec(url: &str) -> Record {
    Record {
        url: url.into(),
        expiry: None,
        created: 0,
        tags: Vec::new(),
        max_visits: None,
        rules: Vec::new(),
        variants: Vec::new(),
        app_ios: None,
        app_android: None,
        folder: None,
        fallback_url: None,
        password_hash: None,
        tenant_id: quark::tenant::DEFAULT_TENANT,
    }
}

async fn mux(url: &str) -> redis::aio::MultiplexedConnection {
    redis::Client::open(url)
        .unwrap()
        .get_multiplexed_async_connection()
        .await
        .unwrap()
}

fn webhooks() -> Arc<quark::webhooks::delivery::WebhookDispatcher> {
    let (tx, _rx) = tokio::sync::mpsc::channel(1);
    Arc::new(quark::webhooks::delivery::WebhookDispatcher::new(
        tx,
        Arc::new(AtomicBool::new(false)),
        Arc::new(AtomicBool::new(false)),
    ))
}

/// Builds one node's `AppState` over a shared store (simulating shared Postgres)
/// and the shared Valkey. The node publishes through its `Invalidator`.
async fn node(store: Arc<dyn Store>, sink: Arc<dyn AnalyticsSink>, url: &str) -> Arc<AppState> {
    let inv = Arc::new(Invalidator {
        conn: Some(mux(url).await),
    });
    let cache = Cache::new(store.clone(), 1000, Some(inv.clone()));
    let host_router = Arc::new(quark::domain_router::HostRouter::new(
        store.clone(),
        None,
        None,
    ));
    let (analytics_tx, _rx) = tokio::sync::mpsc::channel(100);
    Arc::new(AppState {
        oidc: None,
        sheets: None,
        sheets_api: None,
        oidc_configured: false,
        multi_tenant: false,
        tenant_domain_suffix: None,
        cache,
        store,
        key: 0,
        signing_key: [0u8; 32],
        analytics_tx,
        sink,
        admin_token: None,
        ratelimiter: quark::abuse::ratelimit::RateLimiter::disabled(),
        block_private: true,
        public_host: None,
        real_ip_header: "cf-connecting-ip".into(),
        webhooks: webhooks(),
        host_router,
        dns: std::sync::Arc::new(quark::dns::NullDns),
    })
}

#[tokio::test]
async fn cache_invalidation_propagates_to_other_node() {
    let Ok(url) = std::env::var("QUARK_TEST_VALKEY_URL") else {
        eprintln!("skip: QUARK_TEST_VALKEY_URL not set");
        return;
    };
    let dir = tempfile::tempdir().unwrap();
    let (store, sink) = open_backends(dir.path(), false).await.unwrap();
    let id = 987_654u64;
    store
        .put_link(
            quark::tenant::DEFAULT_TENANT,
            id,
            &rec("https://old.example"),
        )
        .await
        .unwrap();

    let node_a = node(store.clone(), sink.clone(), &url).await;
    let node_b = node(store.clone(), sink.clone(), &url).await;
    let _sub = spawn_invalidation_subscriber(url.clone(), node_b.clone());
    tokio::time::sleep(Duration::from_millis(300)).await;

    assert_eq!(
        node_b
            .cache
            .get(quark::tenant::DEFAULT_TENANT, id)
            .await
            .unwrap()
            .unwrap()
            .url,
        "https://old.example"
    );
    store
        .delete_link(quark::tenant::DEFAULT_TENANT, id)
        .await
        .unwrap();

    node_a.cache.invalidate(id).await;

    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        if node_b
            .cache
            .get(quark::tenant::DEFAULT_TENANT, id)
            .await
            .unwrap()
            .is_none()
        {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "B's L1 was not invalidated in time"
        );
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}
