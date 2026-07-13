use quark::analytics::spawn_worker;
use quark::api::{router, AppState};
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
    let (store, sink) = open_backends(std::path::Path::new(&path)).expect("abrir backends");
    let cache = Cache::new(store.clone(), 100_000);
    let (analytics_tx, analytics_rx) = tokio::sync::mpsc::channel(10_000);
    let _worker = spawn_worker(analytics_rx, sink.clone());
    let admin_token = std::env::var("QUARK_ADMIN_TOKEN").ok();
    if admin_token.is_none() {
        eprintln!("AVISO: QUARK_ADMIN_TOKEN não definido — endpoint /stats desligado.");
    }
    let state = Arc::new(AppState {
        cache,
        store,
        key,
        analytics_tx,
        sink,
        admin_token,
    });
    let app = router(state);

    let addr = std::env::var("QUARK_ADDR").unwrap_or_else(|_| "0.0.0.0:8080".into());
    let listener = tokio::net::TcpListener::bind(&addr).await.expect("bind");
    eprintln!("quark ouvindo em {addr}");
    axum::serve(listener, app).await.expect("serve");
}
