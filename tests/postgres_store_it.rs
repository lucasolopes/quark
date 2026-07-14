use quark::store::{postgres::PostgresStore, Record, Store, Variant};
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
        variants: Vec::new(),
    };
    s.put_link(7, &rec).await.unwrap();
    assert_eq!(
        s.get_link(7).await.unwrap().unwrap().url,
        "https://example.com"
    );
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
        variants: Vec::new(),
    };
    assert!(s.put_alias_and_link("promo", 5, &rec).await.unwrap());
    assert!(!s.put_alias_and_link("promo", 9, &rec).await.unwrap());
    assert_eq!(s.get_alias("promo").await.unwrap(), Some(5));
    assert!(s.get_link(9).await.unwrap().is_none());
}

#[tokio::test]
#[serial(pg)]
async fn variants_round_trip_pg() {
    let Some(s) = fresh().await else {
        eprintln!("skip: QUARK_TEST_DATABASE_URL not set");
        return;
    };
    let rec = Record {
        url: "https://default.com".into(),
        expiry: None,
        created: 0,
        variants: vec![
            Variant {
                url: "https://a.com".into(),
                weight: 1,
            },
            Variant {
                url: "https://b.com".into(),
                weight: 3,
            },
        ],
    };
    s.put_link(11, &rec).await.unwrap();
    let got = s.get_link(11).await.unwrap().unwrap();
    assert_eq!(got.variants.len(), 2);
    assert_eq!(got.variants[0].url, "https://a.com");
    assert_eq!(got.variants[0].weight, 1);
    assert_eq!(got.variants[1].url, "https://b.com");
    assert_eq!(got.variants[1].weight, 3);

    // A link created without variants round-trips to an empty vec (not null),
    // matching the JSONB DEFAULT '[]'.
    let plain = Record {
        url: "https://plain.com".into(),
        expiry: None,
        created: 0,
        variants: Vec::new(),
    };
    s.put_link(12, &plain).await.unwrap();
    assert!(s.get_link(12).await.unwrap().unwrap().variants.is_empty());
}
