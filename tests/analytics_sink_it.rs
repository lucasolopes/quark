use quark::analytics::ClickEvent;
use quark::store::open_backends;

fn ev(id: u64, ts: u64) -> ClickEvent {
    ClickEvent {
        id,
        event_id: String::new(),
        ts,
        referer: None,
        country: Some("BR".into()),
        user_agent: Some("iPhone".into()),
        city: None,
        bot: false,
        ip: None,
        fbc: None,
        variant: None,
    }
}

#[tokio::test]
async fn record_and_stats() {
    let dir = tempfile::tempdir().unwrap();
    let (_store, sink) = open_backends(dir.path(), false).await.unwrap();

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
async fn retention_truncates_at_events_max() {
    let dir = tempfile::tempdir().unwrap();
    let (_store, sink) = open_backends(dir.path(), false).await.unwrap();
    for batch in 0..12 {
        let evs: Vec<ClickEvent> = (0..100)
            .map(|i| ev(7, 1_752_300_000 + batch * 100 + i))
            .collect();
        sink.record_batch(&evs).await.unwrap();
    }
    let s = sink.stats(7).await.unwrap().unwrap();
    assert_eq!(s.aggregates.total, 1200);
    assert_eq!(s.recent.len(), 1000);
    assert_eq!(s.recent.last().unwrap().ts, 1_752_300_000 + 11 * 100 + 99);
}
