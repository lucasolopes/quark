//! Microbench in-process do caminho de redirect (sem rede/VM no meio).
//! Mede (1) o redirect completo via router e (2) o custo marginal da captura
//! de analytics adicionada ao 302. Objetivo: responder "quanto a analytics
//! custa no redirect" em nanossegundos, sem o ruído do oha-através-do-Docker.
//!
//! Nota: criterion roda single-thread por iteração, então este número é o
//! custo NÃO-contendido (piso). Contenção do canal mpsc sob N threads reais
//! não aparece aqui — mas o oha também não conseguiu medi-la acima do ruído.

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
                    url: "https://example.com/destino".into(),
                    expiry: None,
                    created: 0,
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

    // (1) redirect completo (router + extractors + resolve + cache-hit +
    // Cache-Control + captura de analytics + resposta 302). Caminho realista.
    c.bench_function("redirect_full_com_analytics", |b| {
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

    // (2) custo marginal SÓ da captura de analytics: montar o ClickEvent
    // (1 alocação de String no user_agent, como um browser real) + try_send.
    // É exatamente o que o 302 ganhou desde a v1 no lado da analytics.
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
