// Test code may panic: the failure IS the signal. clippy.toml covers items
// under #[test]/#[cfg(test)], but not the file-level helpers (fixtures,
// builders), which are most of what this file holds.
#![allow(clippy::unwrap_used)]

use quark::analytics::AnalyticsSink;
use quark::api::AppState;
use quark::cache::Cache;
use quark::invalidate::{spawn_invalidation_subscriber, Invalidator, INVALIDATION_CHANNEL};
use quark::store::postgres::PostgresStore;
use quark::store::{open_backends, Record, Store};
use serial_test::file_serial;
use std::sync::atomic::AtomicBool;
use std::sync::Arc;
use std::time::{Duration, Instant};

mod common;

/// These tests need a live Valkey/Redis. They are skipped (returning early)
/// unless `QUARK_TEST_VALKEY_URL` is set, mirroring `tests/valkey_tier_it.rs`.
/// They cover the cross-node path end to end: node A's request-path
/// `invalidate` publishes on `quark:invalidate`, node B's dedicated subscriber
/// receives it and drops B's stale L1.
/// Blocks until Valkey reports at least one subscriber on the invalidation
/// channel, so a publish from node A cannot race the subscriber's connect and
/// be dropped. Polls `PUBSUB NUMSUB` rather than sleeping a guessed interval:
/// the subscriber count is the condition the test actually depends on.
async fn wait_for_subscriber(url: &str) {
    let client = redis::Client::open(url).unwrap();
    let mut conn = client.get_multiplexed_async_connection().await.unwrap();
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        let (_channel, count): (String, u64) = redis::cmd("PUBSUB")
            .arg("NUMSUB")
            .arg(INVALIDATION_CHANNEL)
            .query_async(&mut conn)
            .await
            .unwrap();
        if count >= 1 {
            return;
        }
        assert!(
            Instant::now() < deadline,
            "no subscriber on {INVALIDATION_CHANNEL} within the deadline"
        );
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

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
    common::TestState::new(store, sink)
        .cache(cache)
        .host_router(host_router)
        .analytics_tx(analytics_tx)
        .webhooks(webhooks())
        .key(0)
        .build()
}

/// Like `node`, but wires a real `Invalidator` into the `HostRouter` too (the
/// cache-only `node` above always passes `None` there), so `host_router
/// .invalidate` actually publishes.
async fn node_with_host_invalidator(
    store: Arc<dyn Store>,
    sink: Arc<dyn AnalyticsSink>,
    url: &str,
) -> Arc<AppState> {
    let cache_inv = Arc::new(Invalidator {
        conn: Some(mux(url).await),
    });
    let host_inv = Arc::new(Invalidator {
        conn: Some(mux(url).await),
    });
    let cache = Cache::new(store.clone(), 1000, Some(cache_inv));
    let host_router = Arc::new(quark::domain_router::HostRouter::new(
        store.clone(),
        None,
        Some(host_inv),
    ));
    let (analytics_tx, _rx) = tokio::sync::mpsc::channel(100);
    common::TestState::new(store, sink)
        .cache(cache)
        .host_router(host_router)
        .analytics_tx(analytics_tx)
        .webhooks(webhooks())
        .key(0)
        .build()
}

#[tokio::test]
#[file_serial]
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
    wait_for_subscriber(&url).await;

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

/// LUC-50: mirrors `cache_invalidation_propagates_to_other_node` for the
/// `HostRouter`. Node A's `host_router.invalidate` publishes `host:<name>`;
/// node B's dedicated subscriber consumes it via `invalidate_local` (no
/// re-publish) and drops B's stale `HostRouter` L1 entry.
#[tokio::test]
#[file_serial]
async fn host_invalidation_propagates_to_other_node() {
    let Ok(url) = std::env::var("QUARK_TEST_VALKEY_URL") else {
        eprintln!("skip: QUARK_TEST_VALKEY_URL not set");
        return;
    };
    // Domains are a cloud feature; LMDB's `put_domain` is `Unsupported`, so this
    // test needs a Postgres-backed store. Skip when Postgres is not configured.
    let Ok(db) = std::env::var("QUARK_TEST_DATABASE_URL") else {
        eprintln!("skip: QUARK_TEST_DATABASE_URL not set");
        return;
    };
    let pg = Arc::new(PostgresStore::open(&db, true).await.unwrap());
    pg.reset_for_tests().await.unwrap();
    let store: Arc<dyn Store> = pg.clone();
    let sink: Arc<dyn AnalyticsSink> = pg;
    let host = "go.acme.example";
    store
        .put_domain(&quark::domain::Domain {
            id: 1,
            tenant_id: quark::tenant::TenantId(1),
            host: host.into(),
            token: "tok".into(),
            status: quark::domain::DomainStatus::Verified,
            created: 0,
            verified_at: Some(0),
        })
        .await
        .unwrap();

    let node_a = node_with_host_invalidator(store.clone(), sink.clone(), &url).await;
    let node_b = node_with_host_invalidator(store.clone(), sink.clone(), &url).await;
    let _sub = spawn_invalidation_subscriber(url.clone(), node_b.clone());
    wait_for_subscriber(&url).await;

    assert_eq!(
        node_b.host_router.resolve(host).await.map(|r| r.domain_id),
        Some(1)
    );
    store
        .delete_domain(quark::tenant::TenantId(1), 1)
        .await
        .unwrap();

    node_a.host_router.invalidate(host).await;

    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        if node_b.host_router.resolve(host).await.is_none() {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "B's HostRouter L1 was not invalidated in time"
        );
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}
