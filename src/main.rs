use quark::analytics::spawn_worker;
use quark::api::{router, AppState};
use quark::cache::valkey::ValkeyTier;
use quark::cache::Cache;
use quark::invalidate::{spawn_invalidation_subscriber, Invalidator, INVALIDATION_CHANNEL};
use quark::store::open_backends;
use quark::webhooks::delivery::{
    spawn_webhook_relay, spawn_webhook_worker, WebhookDispatcher, DELIVERY_TIMEOUT_SECS,
    WEBHOOK_CHANNEL_CAPACITY,
};
use std::sync::atomic::AtomicBool;
use std::sync::Arc;

/// L1 cache capacity (max number of entries held in memory).
const CACHE_CAPACITY: u64 = 100_000;
/// Analytics channel capacity (buffered `ClickEvent`s before backpressure).
const ANALYTICS_CHANNEL_CAPACITY: usize = 10_000;

#[tokio::main]
async fn main() {
    let strict_cluster = std::env::var("QUARK_STRICT_CLUSTER")
        .map(|v| !v.is_empty())
        .unwrap_or(false);
    if let Err(msg) = quark::cluster::cluster_preflight(
        strict_cluster,
        std::env::var("QUARK_DATABASE_URL").is_ok(),
        std::env::var("QUARK_VALKEY_URL").is_ok(),
    ) {
        eprintln!("FATAL: {msg}");
        std::process::exit(1);
    }
    let path = std::env::var("QUARK_DATA").unwrap_or_else(|_| "./data".into());
    let key = std::env::var("QUARK_KEY")
        .ok()
        .and_then(|s| s.parse::<u64>().ok())
        .unwrap_or_else(|| {
            eprintln!("WARNING: QUARK_KEY not set — using dev key. DO NOT use in production.");
            0x9E3779B97F4A7C15
        });
    let (store, sink) = open_backends(std::path::Path::new(&path))
        .await
        .expect("open backends");
    eprintln!(
        "backend: {}",
        if std::env::var("QUARK_DATABASE_URL").is_ok() {
            "postgres"
        } else {
            "lmdb"
        }
    );
    eprintln!(
        "analytics sink: {}",
        if std::env::var("QUARK_CLICKHOUSE_URL").is_ok() {
            "clickhouse"
        } else if std::env::var("QUARK_DATABASE_URL").is_ok() {
            "postgres"
        } else {
            "lmdb(embedded)"
        }
    );
    match std::env::var("QUARK_NODE_ID") {
        Ok(n) if !n.is_empty() && std::env::var("QUARK_DATABASE_URL").is_ok() => {
            eprintln!(
                "WARNING: QUARK_NODE_ID={n} ignored on the Postgres backend (node-id is LMDB-only)"
            );
        }
        Ok(n) if !n.is_empty() => {
            eprintln!("========================================================================");
            eprintln!(
                "WARNING: QUARK_NODE_ID={n} set on the LMDB backend (no QUARK_DATABASE_URL)."
            );
            eprintln!("  LMDB stores are per-node: each replica keeps its OWN file and replicas");
            eprintln!("  do NOT share links. A redirect that lands on a node without the link");
            eprintln!("  returns 404. node-id only partitions the id space (8+32 bits) so codes");
            eprintln!("  do not collide; it does NOT make this a shared multi-node store.");
            eprintln!("  True multi-node needs the Postgres backend (set QUARK_DATABASE_URL).");
            eprintln!("  The node id MUST be unique per replica (e.g. a StatefulSet ordinal);");
            eprintln!("  quark cannot detect a duplicate and a collision silently reuses ids.");
            eprintln!("========================================================================");
        }
        _ => {}
    }
    let control_conn: Option<redis::aio::MultiplexedConnection> =
        match std::env::var("QUARK_VALKEY_URL").ok() {
            Some(url) => match redis::Client::open(url) {
                Ok(client) => client.get_multiplexed_async_connection().await.ok(),
                Err(_) => None,
            },
            None => None,
        };
    let invalidator: Option<Arc<Invalidator>> = control_conn
        .clone()
        .map(|conn| Arc::new(Invalidator { conn: Some(conn) }));

    let cache = match std::env::var("QUARK_VALKEY_URL").ok() {
        Some(url) => {
            match ValkeyTier::open(&url).await {
                Ok(tier) => {
                    let shown = url.rsplit('@').next().unwrap_or(&url);
                    eprintln!("L2 Valkey enabled: {shown}");
                    Cache::with_l2(
                        store.clone(),
                        CACHE_CAPACITY,
                        Arc::new(tier),
                        quark::cache::L1_TTL_SECS,
                        quark::cache::L2_TTL_SECS,
                        invalidator.clone(),
                    )
                }
                Err(e) => {
                    eprintln!("WARNING: failed to connect to Valkey ({e}); continuing with L1+store only.");
                    Cache::new(store.clone(), CACHE_CAPACITY, invalidator.clone())
                }
            }
        }
        None => Cache::new(store.clone(), CACHE_CAPACITY, invalidator.clone()),
    };
    let (analytics_tx, analytics_rx) = tokio::sync::mpsc::channel(ANALYTICS_CHANNEL_CAPACITY);
    let pixel_client = reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .timeout(std::time::Duration::from_secs(5))
        .build()
        .expect("build pixel forwarding client");
    let _worker = spawn_worker(
        analytics_rx,
        sink.clone(),
        store.clone(),
        pixel_client,
        key,
        quark::pixel::PixelBases::default(),
    );
    let admin_token = std::env::var("QUARK_ADMIN_TOKEN").ok();
    if admin_token.is_none() {
        eprintln!("WARNING: QUARK_ADMIN_TOKEN not set — /stats endpoint disabled.");
    }
    if std::env::var("QUARK_ACCESS_LOG").is_err() {
        eprintln!("per-request access log disabled (set QUARK_ACCESS_LOG=1 to enable)");
    }

    let per_min: u32 = std::env::var("QUARK_RATELIMIT_PER_MIN")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(0);
    let ratelimiter = match (per_min, control_conn.clone()) {
        (0, _) => quark::abuse::ratelimit::RateLimiter::disabled(),
        (n, Some(conn)) => quark::abuse::ratelimit::RateLimiter::valkey(n, conn),
        (n, None) => quark::abuse::ratelimit::RateLimiter::memory(n),
    };
    if per_min == 0 {
        eprintln!("rate-limit disabled (set QUARK_RATELIMIT_PER_MIN=n to enable)");
    } else {
        eprintln!(
            "rate-limit: {per_min}/min per IP ({})",
            if control_conn.is_some() {
                "global via Valkey"
            } else {
                "per replica (memory)"
            }
        );
    }
    let block_private = std::env::var("QUARK_BLOCK_PRIVATE")
        .map(|v| v != "0")
        .unwrap_or(true);
    let public_host = std::env::var("QUARK_PUBLIC_HOST").ok();
    let real_ip_header =
        std::env::var("QUARK_REAL_IP_HEADER").unwrap_or_else(|_| "cf-connecting-ip".to_string());

    let (wh_tx, wh_rx) = tokio::sync::mpsc::channel(WEBHOOK_CHANNEL_CAPACITY);
    let clicked = Arc::new(AtomicBool::new(false));
    let expired = Arc::new(AtomicBool::new(false));
    spawn_webhook_worker(wh_rx, store.clone(), clicked.clone(), expired.clone());
    let dispatcher = WebhookDispatcher::new(wh_tx, clicked, expired);
    let webhooks = if std::env::var("QUARK_DATABASE_URL").is_ok() {
        let relay_client = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(DELIVERY_TIMEOUT_SECS))
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .expect("build webhook relay client");
        spawn_webhook_relay(store.clone(), relay_client);
        eprintln!(
            "webhook delivery: durable Postgres outbox + leased relay (lifecycle events); clicked/expired best-effort in-memory"
        );
        Arc::new(dispatcher.with_outbox(store.clone()))
    } else {
        eprintln!("webhook delivery: in-memory best-effort channel (LMDB backend)");
        Arc::new(dispatcher)
    };

    let state = Arc::new(AppState {
        cache,
        store,
        key,
        analytics_tx,
        sink,
        admin_token,
        ratelimiter,
        block_private,
        public_host,
        real_ip_header,
        webhooks,
    });
    match std::env::var("QUARK_VALKEY_URL").ok() {
        Some(url) => {
            eprintln!("cross-node invalidation: pub/sub subscriber on {INVALIDATION_CHANNEL}");
            let _sub = spawn_invalidation_subscriber(url, state.clone());
        }
        None => eprintln!("cross-node invalidation: disabled (no QUARK_VALKEY_URL)"),
    }

    let app = router(state);

    let addr = std::env::var("QUARK_ADDR").unwrap_or_else(|_| "0.0.0.0:8080".into());
    let listener = tokio::net::TcpListener::bind(&addr).await.expect("bind");
    eprintln!("quark listening on {addr}");
    axum::serve(
        listener,
        app.into_make_service_with_connect_info::<std::net::SocketAddr>(),
    )
    .await
    .expect("serve");
}
