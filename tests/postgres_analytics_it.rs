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
#[serial(pg)]
async fn record_and_stats_pg() {
    let Some(s) = fresh().await else {
        eprintln!("skip: QUARK_TEST_DATABASE_URL not set");
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
async fn retention_pg() {
    let Some(s) = fresh().await else {
        eprintln!("skip: QUARK_TEST_DATABASE_URL not set");
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

#[tokio::test]
#[serial(pg)]
async fn record_batch_concurrent_no_lost_updates() {
    let url = match std::env::var("QUARK_TEST_DATABASE_URL") {
        Ok(u) => u,
        Err(_) => return,
    };
    let s0 = PostgresStore::open(&url).await.unwrap();
    s0.reset_for_tests().await.unwrap();
    let s1 = std::sync::Arc::new(PostgresStore::open(&url).await.unwrap());
    let s2 = std::sync::Arc::new(PostgresStore::open(&url).await.unwrap());
    let n = 50u64;
    let t1 = {
        let s = s1.clone();
        tokio::spawn(async move {
            for i in 0..n {
                s.record_batch(&[ev(42, 1_752_300_000 + i)]).await.unwrap();
            }
        })
    };
    let t2 = {
        let s = s2.clone();
        tokio::spawn(async move {
            for i in 0..n {
                s.record_batch(&[ev(42, 1_752_400_000 + i)]).await.unwrap();
            }
        })
    };
    t1.await.unwrap();
    t2.await.unwrap();
    let st = s0.stats(42).await.unwrap().unwrap();
    assert_eq!(st.aggregates.total, 2 * n);
}
