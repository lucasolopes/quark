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

fn ev_ua(id: u64, ts: u64, country: &str, ua: &str) -> ClickEvent {
    ClickEvent {
        id,
        event_id: String::new(),
        ts,
        referer: None,
        country: Some(country.into()),
        user_agent: Some(ua.into()),
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
async fn per_dimension_aggregation_across_batches_pg() {
    let Some(s) = fresh().await else {
        eprintln!("skip: QUARK_TEST_DATABASE_URL not set");
        return;
    };
    s.record_batch(&[
        ev_ua(3, 1_752_300_000, "BR", "Mozilla/5.0 (iPhone)"),
        ev_ua(3, 1_752_300_050, "US", "Mozilla/5.0 (Windows NT 10.0)"),
    ])
    .await
    .unwrap();
    s.record_batch(&[ev_ua(3, 1_752_300_100, "BR", "Mozilla/5.0 (iPhone)")])
        .await
        .unwrap();
    let st = s.stats(3).await.unwrap().unwrap();
    assert_eq!(st.aggregates.total, 3);
    assert_eq!(st.aggregates.per_country.get("BR"), Some(&2));
    assert_eq!(st.aggregates.per_country.get("US"), Some(&1));
    assert_eq!(st.aggregates.per_device.get("Mobile"), Some(&2));
    assert_eq!(st.aggregates.per_device.get("Desktop"), Some(&1));
    assert_eq!(st.aggregates.first_ts, 1_752_300_000);
    assert_eq!(st.aggregates.last_ts, 1_752_300_100);
    assert_eq!(st.aggregates.per_day.values().sum::<u64>(), 3);
}

#[tokio::test]
#[serial(pg)]
async fn bots_counted_in_total_excluded_from_breakdowns_pg() {
    let Some(s) = fresh().await else {
        eprintln!("skip: QUARK_TEST_DATABASE_URL not set");
        return;
    };
    s.record_batch(&[
        ev_ua(9, 1_752_300_000, "BR", "Mozilla/5.0 (iPhone)"),
        ev_ua(9, 1_752_300_050, "US", "Mozilla/5.0 (Windows NT 10.0)"),
        ev_ua(9, 1_752_300_100, "JP", "Googlebot/2.1"),
    ])
    .await
    .unwrap();
    let st = s.stats(9).await.unwrap().unwrap();
    assert_eq!(st.aggregates.total, 3);
    assert_eq!(st.aggregates.bots, 1);
    assert!(
        !st.aggregates.per_country.contains_key("JP"),
        "bot's country must not appear in per_country"
    );
    assert_eq!(st.aggregates.per_country.get("BR"), Some(&1));
    assert_eq!(st.aggregates.per_country.get("US"), Some(&1));
    assert_eq!(st.aggregates.per_device.values().sum::<u64>(), 2);
}

#[tokio::test]
#[serial(pg)]
async fn retention_keeps_newest_pg() {
    let Some(s) = fresh().await else {
        eprintln!("skip: QUARK_TEST_DATABASE_URL not set");
        return;
    };
    for b in 0..12u64 {
        let evs: Vec<ClickEvent> = (0..100)
            .map(|i| ev(11, 1_752_300_000 + b * 100 + i))
            .collect();
        s.record_batch(&evs).await.unwrap();
    }
    let st = s.stats(11).await.unwrap().unwrap();
    assert_eq!(st.recent.len(), 1000);
    let newest_ts = 1_752_300_000 + 11 * 100 + 99;
    let oldest_kept_ts = newest_ts - 999;
    assert_eq!(
        st.recent.last().unwrap().ts,
        newest_ts,
        "newest event must be retained"
    );
    assert_eq!(
        st.recent.first().unwrap().ts,
        oldest_kept_ts,
        "recent must hold the newest EVENTS_MAX, oldest dropped"
    );
}

#[tokio::test]
#[serial(pg)]
async fn stats_none_when_empty_pg() {
    let Some(s) = fresh().await else {
        eprintln!("skip: QUARK_TEST_DATABASE_URL not set");
        return;
    };
    assert!(s.stats(12345).await.unwrap().is_none());
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
