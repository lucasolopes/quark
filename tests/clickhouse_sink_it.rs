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
        bot: false,
        ip: None,
        fbc: None,
        variant: None,
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
        bot: false,
        ip: None,
        fbc: None,
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
    assert_eq!(st.aggregates.total, 3, "total counts the bot click too");
    assert_eq!(st.aggregates.bots, 1, "curl/8 is flagged as a bot");
    assert_eq!(st.aggregates.per_country.get("BR"), Some(&2));
    assert_eq!(
        st.aggregates.per_country.get("US"),
        None,
        "bot click excluded from the human-only breakdown"
    );
    assert_eq!(st.aggregates.per_device.get("Mobile"), Some(&1));
    assert_eq!(st.aggregates.per_device.get("Desktop"), Some(&1));
    assert_eq!(
        st.aggregates.per_device.get("Other"),
        None,
        "bot click excluded from the human-only breakdown"
    );
    assert_eq!(st.recent.len(), 3, "recent includes bots too");
    assert!(!st.recent[0].bot);
    assert!(!st.recent[1].bot);
    assert!(st.recent[2].bot, "curl/8 recent event flagged as bot");
    assert!(s.stats(999).await.unwrap().is_none());
}

#[tokio::test]
#[serial(ch)]
async fn bot_filter_ch() {
    let Some(s) = fresh().await else {
        eprintln!("skip: QUARK_TEST_CLICKHOUSE_URL not set");
        return;
    };
    s.record_batch(&[
        ev(3, 1_752_300_000, "BR", "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/120.0.0.0 Safari/537.36"),
        ev(
            3,
            1_752_300_050,
            "JP",
            "Mozilla/5.0 (compatible; Googlebot/2.1; +http://www.google.com/bot.html)",
        ),
        ev(3, 1_752_300_100, "DE", "curl/8.4.0"),
    ])
    .await
    .unwrap();
    let st = s.stats(3).await.unwrap().unwrap();
    assert_eq!(st.aggregates.total, 3, "total counts bots too");
    assert_eq!(st.aggregates.bots, 2, "Googlebot and curl are both bots");
    assert_eq!(st.aggregates.per_country.get("BR"), Some(&1));
    assert_eq!(
        st.aggregates.per_country.get("JP"),
        None,
        "Googlebot excluded from human breakdown"
    );
    assert_eq!(
        st.aggregates.per_country.get("DE"),
        None,
        "curl excluded from human breakdown"
    );
    assert_eq!(st.recent.len(), 3);
    assert!(!st.recent[0].bot, "Chrome recent event is not a bot");
    assert!(st.recent[1].bot, "Googlebot recent event flagged as bot");
    assert!(st.recent[2].bot, "curl recent event flagged as bot");
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
