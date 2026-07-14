use quark::analytics::clickhouse::ClickHouseSink;
use quark::analytics::{AnalyticsSink, ClickEvent};
use serial_test::serial;

async fn fresh() -> Option<ClickHouseSink> {
    let url = std::env::var("QUARK_TEST_CLICKHOUSE_URL").ok()?;
    let s = ClickHouseSink::open(&url).await.unwrap();
    s.reset_for_tests().await.unwrap();
    Some(s)
}
fn ev(id: u64, ts: u64, c: &str, ua: &str) -> ClickEvent {
    ClickEvent {
        id,
        ts,
        referer: None,
        country: Some(c.into()),
        user_agent: Some(ua.into()),
        variant: None,
    }
}

#[tokio::test]
#[serial(ch)]
async fn record_and_stats_ch() {
    let Some(s) = fresh().await else {
        eprintln!("skip: QUARK_TEST_CLICKHOUSE_URL not set");
        return;
    };
    s.record_batch(&[
        ev(1, 1_752_300_000, "BR", "iPhone"),
        ev(1, 1_752_300_050, "BR", "Windows NT 10.0"),
        ev(1, 1_752_400_000, "US", "curl/8"),
    ])
    .await
    .unwrap();
    let st = s.stats(1).await.unwrap().unwrap();
    assert_eq!(st.aggregates.total, 3);
    assert_eq!(st.aggregates.per_country.get("BR"), Some(&2));
    assert_eq!(st.aggregates.per_country.get("US"), Some(&1));
    assert_eq!(st.aggregates.per_device.get("Mobile"), Some(&1));
    assert_eq!(st.aggregates.per_device.get("Desktop"), Some(&1));
    assert_eq!(st.aggregates.per_device.get("Other"), Some(&1));
    assert_eq!(st.recent.len(), 3);
    assert!(s.stats(999).await.unwrap().is_none());
}

#[tokio::test]
#[serial(ch)]
async fn recent_limits_to_n_ch() {
    let Some(s) = fresh().await else {
        return;
    };
    let evs: Vec<ClickEvent> = (0..1200u64)
        .map(|i| ev(7, 1_752_300_000 + i, "BR", "iPhone"))
        .collect();
    s.record_batch(&evs).await.unwrap();
    let st = s.stats(7).await.unwrap().unwrap();
    assert_eq!(st.aggregates.total, 1200);
    assert_eq!(st.recent.len(), 1000);
}

#[tokio::test]
#[serial(ch)]
async fn per_variant_aggregates_and_recent_ch() {
    let Some(s) = fresh().await else {
        eprintln!("skip: QUARK_TEST_CLICKHOUSE_URL not set");
        return;
    };
    let mut a = ev(20, 1_752_300_000, "BR", "iPhone");
    a.variant = Some(0);
    let mut b = ev(20, 1_752_300_001, "BR", "iPhone");
    b.variant = Some(0);
    let mut c = ev(20, 1_752_300_002, "BR", "iPhone");
    c.variant = Some(1);
    let mut d = ev(20, 1_752_300_003, "BR", "iPhone");
    d.variant = None;
    s.record_batch(&[a, b, c, d]).await.unwrap();
    let st = s.stats(20).await.unwrap().unwrap();
    assert_eq!(st.aggregates.per_variant.get("0"), Some(&2));
    assert_eq!(st.aggregates.per_variant.get("1"), Some(&1));
    assert_eq!(st.aggregates.per_variant.get("-1"), None);
    let variants: Vec<Option<u32>> = st.recent.iter().map(|e| e.variant).collect();
    assert!(variants.contains(&Some(0)));
    assert!(variants.contains(&Some(1)));
    assert!(variants.contains(&None));
}
