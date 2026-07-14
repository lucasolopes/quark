//! In-process microbench of the redirect path (no network/VM in between).
//! Measures (1) the full redirect via the router and (2) the marginal cost of the
//! analytics capture added to the 302. Goal: answer "how much does analytics
//! cost on the redirect" in nanoseconds, without the noise of oha-through-Docker.
//!
//! Note: criterion runs single-threaded per iteration, so this number is the
//! UNcontended cost (floor). Contention on the mpsc channel under N real threads
//! doesn't show up here — but oha couldn't measure it above the noise either.

use criterion::{black_box, criterion_group, criterion_main, Criterion};
use quark::abuse::blocklist::Blocklist;
use quark::abuse::ratelimit::RateLimiter;
use quark::analytics::{spawn_worker, ClickEvent};
use quark::api::{router, AppState};
use quark::cache::Cache;
use quark::codec::to_base62;
use quark::permute::encode;
use quark::store::{open_backends, Record};
use std::sync::Arc;

use axum::body::Body;
use axum::http::Request;
use tower::ServiceExt;

fn bench(c: &mut Criterion) {
    let rt = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(4)
        .enable_all()
        .build()
        .unwrap();
    let key = 0x9E3779B97F4A7C15u64;
    let dir = tempfile::tempdir().unwrap();

    let (state, code, tx, _worker) = rt.block_on(async {
        let (store, sink) = open_backends(dir.path()).await.unwrap();
        store
            .put_link(
                1,
                &Record {
                    url: "https://example.com/destination".into(),
                    expiry: None,
                    created: 0,
                    rules: Vec::new(),
                },
            )
            .await
            .unwrap();
        let cache = Cache::new(store.clone(), 100_000);
        let (tx, rx) = tokio::sync::mpsc::channel::<ClickEvent>(10_000);
        let worker = spawn_worker(rx, sink.clone());
        let state = Arc::new(AppState {
            cache,
            store: store.clone(),
            key,
            analytics_tx: tx.clone(),
            sink,
            admin_token: None,
            ratelimiter: RateLimiter::disabled(),
            blocklist: Blocklist::new(store, None, 60),
            block_private: true,
            public_host: None,
            real_ip_header: "cf-connecting-ip".to_string(),
        });
        let code = to_base62(encode(1, key));
        (state, code, tx, worker)
    });

    let app = router(state);
    let path = format!("/{code}");

    c.bench_function("redirect_full_with_analytics", |b| {
        b.to_async(&rt).iter(|| {
            let app = app.clone();
            let path = path.clone();
            async move {
                let resp = app
                    .oneshot(
                        Request::get(&path)
                            .header("user-agent", "Mozilla/5.0 (X11; Linux x86_64)")
                            .body(Body::empty())
                            .unwrap(),
                    )
                    .await
                    .unwrap();
                black_box(resp);
            }
        })
    });

    c.bench_function("analytics_capture", |b| {
        b.to_async(&rt).iter(|| {
            let tx = tx.clone();
            async move {
                let ev = ClickEvent {
                    id: 1,
                    ts: 0,
                    referer: None,
                    country: None,
                    user_agent: Some("Mozilla/5.0 (X11; Linux x86_64)".to_string()),
                };
                let _ = tx.try_send(black_box(ev));
            }
        })
    });
}

criterion_group!(benches, bench);
criterion_main!(benches);
