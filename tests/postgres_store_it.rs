use quark::store::{postgres::PostgresStore, Record, Rule, RuleField, Store, Variant};
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
        max_visits: None,
        rules: Vec::new(),
        variants: Vec::new(),
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
async fn rules_round_trip_pg() {
    let Some(s) = fresh().await else {
        eprintln!("skip: QUARK_TEST_DATABASE_URL not set");
        return;
    };
    let rec = Record {
        url: "https://default.example".into(),
        expiry: None,
        created: 0,
        tags: Vec::new(),
        max_visits: None,
        rules: vec![
            Rule {
                field: RuleField::Country,
                values: vec!["BR".into()],
                to: "https://br.example".into(),
            },
            Rule {
                field: RuleField::Device,
                values: vec!["Mobile".into()],
                to: "https://m.example".into(),
            },
        ],
        variants: Vec::new(),
        app_ios: None,
        app_android: None,
    };
    s.put_link(42, &rec).await.unwrap();
    let got = s.get_link(42).await.unwrap().unwrap();
    assert_eq!(got.rules, rec.rules);
}

#[tokio::test]
#[serial(pg)]
async fn link_without_rules_round_trips_to_empty_vec_pg() {
    let Some(s) = fresh().await else {
        eprintln!("skip: QUARK_TEST_DATABASE_URL not set");
        return;
    };
    let rec = Record {
        url: "https://no-rules.example".into(),
        expiry: None,
        created: 0,
        tags: Vec::new(),
        max_visits: None,
        rules: Vec::new(),
        variants: Vec::new(),
        app_ios: None,
        app_android: None,
    };
    s.put_link(43, &rec).await.unwrap();
    let got = s.get_link(43).await.unwrap().unwrap();
    assert!(got.rules.is_empty());
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
        max_visits: None,
        rules: Vec::new(),
        variants: Vec::new(),
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
async fn tags_round_trip_filter_and_distinct_pg() {
    let Some(s) = fresh().await else {
        return;
    };
    let rec = |u: &str, tags: &[&str]| Record {
        url: u.into(),
        expiry: None,
        created: 0,
        tags: tags.iter().map(|t| t.to_string()).collect(),
        max_visits: None,
        rules: Vec::new(),
        variants: Vec::new(),
        app_ios: None,
        app_android: None,
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
        tags: Vec::new(),
        max_visits: Some(5),
        rules: Vec::new(),
        variants: Vec::new(),
        app_ios: None,
        app_android: None,
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
        tags: Vec::new(),
        max_visits: None,
        rules: Vec::new(),
        variants: Vec::new(),
        app_ios: None,
        app_android: None,
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
        tags: Vec::new(),
        max_visits: None,
        rules: Vec::new(),
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
        app_ios: None,
        app_android: None,
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
        tags: Vec::new(),
        max_visits: None,
        rules: Vec::new(),
        variants: Vec::new(),
        app_ios: None,
        app_android: None,
    };
    s.put_link(12, &plain).await.unwrap();
    assert!(s.get_link(12).await.unwrap().unwrap().variants.is_empty());
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
