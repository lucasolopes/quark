use quark::cache::valkey::ValkeyTier;
use quark::cache::CacheTier;
use quark::store::Record;

// Só roda se QUARK_TEST_VALKEY_URL estiver setado (ex.: redis://127.0.0.1:6379).
#[tokio::test]
async fn set_get_round_trip() {
    let Ok(url) = std::env::var("QUARK_TEST_VALKEY_URL") else {
        eprintln!("skip: QUARK_TEST_VALKEY_URL não setado");
        return;
    };
    let tier = ValkeyTier::open(&url).await.unwrap();
    let id = 424242u64;
    assert!(tier.get(id).await.unwrap().is_none() || true); // pode ter lixo de rodada anterior
    let rec = Record {
        url: "https://example.com/valkey".into(),
        expiry: None,
        created: 1,
    };
    tier.set(id, &rec, 60).await.unwrap();
    let got = tier.get(id).await.unwrap().unwrap();
    assert_eq!(got.url, "https://example.com/valkey");
}
