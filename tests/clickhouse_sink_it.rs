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
        city: None,
    }
}
fn ev_full(id: u64, ts: u64, ua: &str, referer: Option<&str>, city: Option<&str>) -> ClickEvent {
    ClickEvent {
        id,
        ts,
        referer: referer.map(String::from),
        country: Some("BR".into()),
        user_agent: Some(ua.into()),
        city: city.map(String::from),
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
async fn os_browser_referer_city_ch() {
    let Some(s) = fresh().await else {
        eprintln!("skip: QUARK_TEST_CLICKHOUSE_URL not set");
        return;
    };
    s.record_batch(&[
        ev_full(
            2,
            1_752_300_000,
            "Mozilla/5.0 (iPhone; CPU iPhone OS 17_0) AppleWebKit/605.1.15 (KHTML, like Gecko) Version/17.0 Mobile/15E148 Safari/604.1",
            Some("https://news.ycombinator.com/x"),
            Some("Sao Paulo"),
        ),
        ev_full(
            2,
            1_752_300_050,
            "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/120.0.0.0 Safari/537.36",
            None,
            None,
        ),
        ev_full(
            2,
            1_752_300_100,
            "Mozilla/5.0 (Windows NT 10.0; Win64; x64; rv:121.0) Gecko/20100101 Firefox/121.0",
            Some("https://news.ycombinator.com/y"),
            None,
        ),
    ])
    .await
    .unwrap();
    let st = s.stats(2).await.unwrap().unwrap();
    assert_eq!(st.aggregates.per_os.get("iOS"), Some(&1));
    assert_eq!(st.aggregates.per_os.get("Windows"), Some(&2));
    assert_eq!(st.aggregates.per_browser.get("Safari"), Some(&1));
    assert_eq!(st.aggregates.per_browser.get("Chrome"), Some(&1));
    assert_eq!(st.aggregates.per_browser.get("Firefox"), Some(&1));
    assert_eq!(
        st.aggregates.per_referer.get("news.ycombinator.com"),
        Some(&2)
    );
    assert_eq!(st.aggregates.per_referer.get("direct"), Some(&1));
    assert_eq!(st.aggregates.per_city.get("Sao Paulo"), Some(&1));
    assert_eq!(st.aggregates.per_city.len(), 1, "empty city excluded");
}
