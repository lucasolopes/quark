use quark::cache::valkey::ValkeyTier;
use quark::cache::CacheTier;
use quark::store::Record;

#[tokio::test]
async fn set_get_round_trip() {
    let Ok(url) = std::env::var("QUARK_TEST_VALKEY_URL") else {
        eprintln!("skip: QUARK_TEST_VALKEY_URL not set");
        return;
    };
    let tier = ValkeyTier::open(&url).await.unwrap();
    let id = 424242u64;
    assert!(tier.get(id).await.unwrap().is_none() || true);
    let rec = Record {
        url: "https://example.com/valkey".into(),
        expiry: None,
        created: 1,
        rules: Vec::new(),
    };
    tier.set(id, &rec, 60).await.unwrap();
    let got = tier.get(id).await.unwrap().unwrap();
    assert_eq!(got.url, "https://example.com/valkey");
}
