use quark::analytics::ClickEvent;
use quark::store::open_backends;

fn ev(id: u64, ts: u64) -> ClickEvent {
    ClickEvent {
        id,
        ts,
        referer: None,
        country: Some("BR".into()),
        user_agent: Some("iPhone".into()),
    }
}

#[tokio::test]
async fn record_e_stats() {
    let dir = tempfile::tempdir().unwrap();
    let (_store, sink) = open_backends(dir.path()).await.unwrap();

    sink.record_batch(&[ev(1, 1_752_300_000), ev(1, 1_752_300_050)])
        .await
        .unwrap();
    let s = sink.stats(1).await.unwrap().unwrap();
    assert_eq!(s.aggregates.total, 2);
    assert_eq!(s.aggregates.per_country.get("BR"), Some(&2));
    assert_eq!(s.recent.len(), 2);
    assert!(sink.stats(999).await.unwrap().is_none());
}

#[tokio::test]
async fn retencao_trunca_em_events_max() {
    let dir = tempfile::tempdir().unwrap();
    let (_store, sink) = open_backends(dir.path()).await.unwrap();
    // Grava 1200 eventos pro mesmo id em lotes; recent deve ficar em 1000.
    for batch in 0..12 {
        let evs: Vec<ClickEvent> = (0..100)
            .map(|i| ev(7, 1_752_300_000 + batch * 100 + i))
            .collect();
        sink.record_batch(&evs).await.unwrap();
    }
    let s = sink.stats(7).await.unwrap().unwrap();
    assert_eq!(s.aggregates.total, 1200);
    assert_eq!(s.recent.len(), 1000); // últimos N
                                      // o mais recente sobreviveu; o mais antigo foi truncado
    assert_eq!(s.recent.last().unwrap().ts, 1_752_300_000 + 11 * 100 + 99);
}
