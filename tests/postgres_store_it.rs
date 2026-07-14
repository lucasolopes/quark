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
        tags: Vec::new(),
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
        tags: Vec::new(),
    };
    assert!(s.put_alias_and_link("promo", 5, &rec).await.unwrap());
    assert!(!s.put_alias_and_link("promo", 9, &rec).await.unwrap());
    assert_eq!(s.get_alias("promo").await.unwrap(), Some(5));
    assert!(s.get_link(9).await.unwrap().is_none());
}

#[tokio::test]
#[serial(pg)]
async fn tags_round_trip_filter_and_distinct_pg() {
    let Some(s) = fresh().await else {
        return;
    };
    let rec = |u: &str, tags: &[&str]| Record {
        url: u.into(),
        expiry: None,
        created: 0,
        tags: tags.iter().map(|t| t.to_string()).collect(),
    };
    s.put_link(1, &rec("https://a.com", &["rust", "web"]))
        .await
        .unwrap();
    s.put_link(2, &rec("https://b.com", &["web"]))
        .await
        .unwrap();
    s.put_link(3, &rec("https://c.com", &[])).await.unwrap();

    let got = s.get_link(1).await.unwrap().unwrap();
    assert_eq!(got.tags, vec!["rust".to_string(), "web".to_string()]);

    let filtered = s.list_links(None, 50, Some("rust")).await.unwrap();
    assert_eq!(
        filtered.iter().map(|(id, _)| *id).collect::<Vec<_>>(),
        vec![1]
    );

    let both_web = s.list_links(None, 50, Some("web")).await.unwrap();
    let mut ids: Vec<u64> = both_web.iter().map(|(id, _)| *id).collect();
    ids.sort();
    assert_eq!(ids, vec![1, 2]);

    let mut tags = s.list_tags().await.unwrap();
    tags.sort();
    assert_eq!(tags, vec!["rust".to_string(), "web".to_string()]);
}
