use quark::analytics::spawn_worker;
use quark::api::{router, AppState};
use quark::cache::valkey::ValkeyTier;
use quark::cache::Cache;
use quark::store::open_backends;
use std::sync::Arc;

#[tokio::main]
async fn main() {
    let path = std::env::var("QUARK_DATA").unwrap_or_else(|_| "./data".into());
    let key = std::env::var("QUARK_KEY")
        .ok()
        .and_then(|s| s.parse::<u64>().ok())
        .unwrap_or_else(|| {
            eprintln!("AVISO: QUARK_KEY não definido — usando chave de dev. NÃO use em produção.");
            0x9E3779B97F4A7C15
        });
    let (store, sink) = open_backends(std::path::Path::new(&path))
        .await
        .expect("abrir backends");
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
            "lmdb(embutido)"
        }
    );
    match std::env::var("QUARK_NODE_ID") {
        Ok(n) if !n.is_empty() && std::env::var("QUARK_DATABASE_URL").is_ok() => {
            eprintln!(
                "AVISO: QUARK_NODE_ID={n} ignorado no backend Postgres (node-id é só do LMDB)"
            );
        }
        Ok(n) if !n.is_empty() => {
            eprintln!("node-id LMDB: {n} (espaço de id particionado em 8+32 bits)")
        }
        _ => {}
    }
    let cache = match std::env::var("QUARK_VALKEY_URL").ok() {
        Some(url) => match ValkeyTier::open(&url).await {
            Ok(tier) => {
                let shown = url.rsplit('@').next().unwrap_or(&url);
                eprintln!("L2 Valkey habilitado: {shown}");
                Cache::with_l2(
                    store.clone(),
                    100_000,
                    Arc::new(tier),
                    quark::cache::L1_TTL_SECS,
                    quark::cache::L2_TTL_SECS,
                )
            }
            Err(e) => {
                eprintln!("AVISO: falha ao conectar no Valkey ({e}); seguindo só com L1+store.");
                Cache::new(store.clone(), 100_000)
            }
        },
        None => Cache::new(store.clone(), 100_000),
    };
    let (analytics_tx, analytics_rx) = tokio::sync::mpsc::channel(10_000);
    let _worker = spawn_worker(analytics_rx, sink.clone());
    let admin_token = std::env::var("QUARK_ADMIN_TOKEN").ok();
    if admin_token.is_none() {
        eprintln!("AVISO: QUARK_ADMIN_TOKEN não definido — endpoint /stats desligado.");
    }
    if std::env::var("QUARK_ACCESS_LOG").is_err() {
        eprintln!("access log por request desligado (defina QUARK_ACCESS_LOG=1 para ligar)");
    }

    // --- proteção contra abuso ---
    let control_conn: Option<redis::aio::MultiplexedConnection> =
        match std::env::var("QUARK_VALKEY_URL").ok() {
            Some(url) => match redis::Client::open(url) {
                Ok(client) => client.get_multiplexed_async_connection().await.ok(),
                Err(_) => None,
            },
            None => None,
        };
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
        eprintln!("rate-limit desligado (defina QUARK_RATELIMIT_PER_MIN=n para ligar)");
    } else {
        eprintln!(
            "rate-limit: {per_min}/min por IP ({})",
            if control_conn.is_some() {
                "global via Valkey"
            } else {
                "por réplica (memória)"
            }
        );
    }
    let blocklist_ttl: u64 = std::env::var("QUARK_BLOCKLIST_TTL")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(60);
    let blocklist =
        quark::abuse::blocklist::Blocklist::new(store.clone(), control_conn.clone(), blocklist_ttl);
    let block_private = std::env::var("QUARK_BLOCK_PRIVATE")
        .map(|v| v != "0")
        .unwrap_or(true);
    let public_host = std::env::var("QUARK_PUBLIC_HOST").ok();
    let real_ip_header =
        std::env::var("QUARK_REAL_IP_HEADER").unwrap_or_else(|_| "cf-connecting-ip".to_string());

    let state = Arc::new(AppState {
        cache,
        store,
        key,
        analytics_tx,
        sink,
        admin_token,
        ratelimiter,
        blocklist,
        block_private,
        public_host,
        real_ip_header,
    });
    let app = router(state);

    let addr = std::env::var("QUARK_ADDR").unwrap_or_else(|_| "0.0.0.0:8080".into());
    let listener = tokio::net::TcpListener::bind(&addr).await.expect("bind");
    eprintln!("quark ouvindo em {addr}");
    axum::serve(
        listener,
        app.into_make_service_with_connect_info::<std::net::SocketAddr>(),
    )
    .await
    .expect("serve");
}
