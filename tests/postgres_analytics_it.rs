use quark::analytics::{AnalyticsSink, ClickEvent};
use quark::store::postgres::PostgresStore;
use serial_test::serial;

async fn fresh() -> Option<PostgresStore> {
    let url = std::env::var("QUARK_TEST_DATABASE_URL").ok()?;
    let s = PostgresStore::open(&url).await.unwrap();
    s.reset_for_tests().await.unwrap();
    Some(s)
}
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
#[serial(pg)]
async fn record_e_stats_pg() {
    let Some(s) = fresh().await else {
        eprintln!("skip: sem QUARK_TEST_DATABASE_URL");
        return;
    };
    s.record_batch(&[ev(1, 1_752_300_000), ev(1, 1_752_300_050)])
        .await
        .unwrap();
    let st = s.stats(1).await.unwrap().unwrap();
    assert_eq!(st.aggregates.total, 2);
    assert_eq!(st.aggregates.per_country.get("BR"), Some(&2));
    assert_eq!(st.recent.len(), 2);
    assert!(s.stats(999).await.unwrap().is_none());
}

#[tokio::test]
#[serial(pg)]
async fn retencao_pg() {
    let Some(s) = fresh().await else {
        eprintln!("skip: sem QUARK_TEST_DATABASE_URL");
        return;
    };
    for b in 0..12u64 {
        let evs: Vec<ClickEvent> = (0..100)
            .map(|i| ev(7, 1_752_300_000 + b * 100 + i))
            .collect();
        s.record_batch(&evs).await.unwrap();
    }
    let st = s.stats(7).await.unwrap().unwrap();
    assert_eq!(st.aggregates.total, 1200);
    assert_eq!(st.recent.len(), 1000);
}
