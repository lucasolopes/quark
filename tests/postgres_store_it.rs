use quark::store::{postgres::PostgresStore, Record, Store};
use serial_test::serial;

async fn fresh() -> Option<PostgresStore> {
    let url = std::env::var("QUARK_TEST_DATABASE_URL").ok()?;
    let s = PostgresStore::open(&url).await.unwrap();
    s.reset_for_tests().await.unwrap();
    Some(s)
}

#[tokio::test]
#[serial(pg)]
async fn put_get_link_pg() {
    let Some(s) = fresh().await else {
        eprintln!("skip: QUARK_TEST_DATABASE_URL not set");
        return;
    };
    let rec = Record {
        url: "https://example.com".into(),
        expiry: None,
        created: 100,
        app_ios: Some("https://apps.apple.com/x".into()),
        app_android: None,
    };
    s.put_link(7, &rec).await.unwrap();
    let got = s.get_link(7).await.unwrap().unwrap();
    assert_eq!(got.url, "https://example.com");
    assert_eq!(got.app_ios.as_deref(), Some("https://apps.apple.com/x"));
    assert_eq!(got.app_android, None);
    assert!(s.get_link(999).await.unwrap().is_none());
}

#[tokio::test]
#[serial(pg)]
async fn next_id_increments_pg() {
    let Some(s) = fresh().await else {
        return;
    };
    let a = s.next_id().await.unwrap();
    let b = s.next_id().await.unwrap();
    assert_eq!(b, a + 1);
}

#[tokio::test]
#[serial(pg)]
async fn alias_is_atomic_no_orphan_pg() {
    let Some(s) = fresh().await else {
        return;
    };
    let rec = Record {
        url: "u".into(),
        expiry: None,
        created: 0,
        app_ios: None,
        app_android: None,
    };
    assert!(s.put_alias_and_link("promo", 5, &rec).await.unwrap());
    assert!(!s.put_alias_and_link("promo", 9, &rec).await.unwrap());
    assert_eq!(s.get_alias("promo").await.unwrap(), Some(5));
    assert!(s.get_link(9).await.unwrap().is_none());
}

#[tokio::test]
#[serial(pg)]
async fn wellknown_round_trip_pg() {
    let Some(s) = fresh().await else {
        return;
    };
    assert_eq!(s.get_wellknown("assetlinks.json").await.unwrap(), None);
    let body = r#"{"relation":["delegate_permission/common.handle_all_urls"]}"#;
    s.put_wellknown("assetlinks.json", body).await.unwrap();
    assert_eq!(
        s.get_wellknown("assetlinks.json").await.unwrap(),
        Some(body.to_string())
    );
    s.delete_wellknown("assetlinks.json").await.unwrap();
    assert_eq!(s.get_wellknown("assetlinks.json").await.unwrap(), None);
    s.delete_wellknown("assetlinks.json").await.unwrap();
}
