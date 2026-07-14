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
        max_visits: None,
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
        max_visits: None,
    };
    assert!(s.put_alias_and_link("promo", 5, &rec).await.unwrap());
    assert!(!s.put_alias_and_link("promo", 9, &rec).await.unwrap());
    assert_eq!(s.get_alias("promo").await.unwrap(), Some(5));
    assert!(s.get_link(9).await.unwrap().is_none());
}

#[tokio::test]
#[serial(pg)]
async fn visits_round_trip_pg() {
    let Some(s) = fresh().await else {
        eprintln!("skip: QUARK_TEST_DATABASE_URL not set");
        return;
    };
    let rec = Record {
        url: "https://example.com".into(),
        expiry: None,
        created: 0,
        max_visits: Some(5),
    };
    s.put_link(11, &rec).await.unwrap();
    assert_eq!(s.visits(11).await.unwrap(), 0);
    assert_eq!(s.get_link(11).await.unwrap().unwrap().max_visits, Some(5));
}

#[tokio::test]
#[serial(pg)]
async fn bump_visits_is_atomic_and_increments_pg() {
    let Some(s) = fresh().await else {
        eprintln!("skip: QUARK_TEST_DATABASE_URL not set");
        return;
    };
    let rec = Record {
        url: "https://example.com".into(),
        expiry: None,
        created: 0,
        max_visits: None,
    };
    s.put_link(12, &rec).await.unwrap();
    assert_eq!(s.bump_visits(12).await.unwrap(), 1);
    assert_eq!(s.bump_visits(12).await.unwrap(), 2);
    assert_eq!(s.visits(12).await.unwrap(), 2);

    let s = std::sync::Arc::new(s);
    let mut handles = Vec::new();
    for _ in 0..10 {
        let s2 = s.clone();
        handles.push(tokio::spawn(
            async move { s2.bump_visits(12).await.unwrap() },
        ));
    }
    let mut results = Vec::new();
    for h in handles {
        results.push(h.await.unwrap());
    }
    results.sort();
    assert_eq!(results, (3..=12).collect::<Vec<u64>>());
    assert_eq!(s.visits(12).await.unwrap(), 12);
}
