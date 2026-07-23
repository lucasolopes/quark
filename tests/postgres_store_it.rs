use quark::store::{postgres::PostgresStore, OutboxRow, Record, Rule, RuleField, Store, Variant};
use quark::webhooks::{EventType, SubscriptionKind, WebhookSubscription};
use serial_test::file_serial;
use sqlx::postgres::PgPoolOptions;
use sqlx::{PgPool, Row};

async fn fresh() -> Option<PostgresStore> {
    let url = std::env::var("QUARK_TEST_DATABASE_URL").ok()?;
    let s = PostgresStore::open(&url, false).await.unwrap();
    s.reset_for_tests().await.unwrap();
    Some(s)
}

/// A fresh store plus a raw pool used to inspect `webhook_deliveries` rows the
/// `Store` trait does not expose.
async fn fresh_with_pool() -> Option<(PostgresStore, PgPool)> {
    let url = std::env::var("QUARK_TEST_DATABASE_URL").ok()?;
    let s = PostgresStore::open(&url, false).await.unwrap();
    s.reset_for_tests().await.unwrap();
    let pool = PgPoolOptions::new()
        .max_connections(4)
        .connect(&url)
        .await
        .unwrap();
    Some((s, pool))
}

fn plain_rec(url: &str) -> Record {
    Record {
        url: url.into(),
        expiry: None,
        created: 100,
        tags: Vec::new(),
        max_visits: None,
        rules: Vec::new(),
        variants: Vec::new(),
        app_ios: None,
        app_android: None,
        folder: None,
        fallback_url: None,
        password_hash: None,
        tenant_id: quark::tenant::DEFAULT_TENANT,
    }
}

async fn add_sub(store: &PostgresStore, url: &str) -> WebhookSubscription {
    let id = store
        .next_webhook_id(quark::tenant::DEFAULT_TENANT)
        .await
        .unwrap();
    let sub = WebhookSubscription {
        id,
        url: url.to_string(),
        events: vec![EventType::LinkCreated],
        secret: "whsec_MfKQ9r8GKYqrTwjUPD8ILPZIo2LaLaSw".to_string(),
        active: true,
        created: 1,
        kind: SubscriptionKind::Generic,
        label: None,
        connector_id: None,
        external_id: None,
        last_delivery_at: None,
        last_delivery_status: Default::default(),
    };
    store
        .put_webhook(quark::tenant::DEFAULT_TENANT, &sub)
        .await
        .unwrap();
    sub
}

fn outbox_row(key: &str, sub_id: u64, at: u64) -> OutboxRow {
    OutboxRow {
        delivery_key: key.to_string(),
        subscription_id: sub_id,
        event_type: "link.created".to_string(),
        payload: r#"{"id":"evt_test","type":"link.created"}"#.to_string(),
        created: at,
        next_attempt_at: at,
        tenant_id: quark::tenant::DEFAULT_TENANT,
    }
}

async fn count_deliveries(pool: &PgPool, key: &str) -> i64 {
    sqlx::query("SELECT COUNT(*) AS n FROM webhook_deliveries WHERE delivery_key=$1")
        .bind(key)
        .fetch_one(pool)
        .await
        .unwrap()
        .try_get("n")
        .unwrap()
}

/// The read/write split routes writes to the primary pool and reads to the
/// replica pool. Built via `open_with_replica` with BOTH URLs pointing at the
/// same test DB (CI has no real replica), this proves the routing wiring: a
/// `put_link` on the write pool is visible to a `get_link` on the read pool.
#[tokio::test]
#[file_serial]
async fn open_with_replica_write_then_read_round_trips() {
    let Some(url) = std::env::var("QUARK_TEST_DATABASE_URL").ok() else {
        eprintln!("skip: QUARK_TEST_DATABASE_URL not set");
        return;
    };
    let s = PostgresStore::open_with_replica(&url, &url, false)
        .await
        .unwrap();
    s.reset_for_tests().await.unwrap();
    let rec = plain_rec("https://replica-routed.example");
    s.put_link(quark::tenant::DEFAULT_TENANT, 101, &rec)
        .await
        .unwrap();
    let got = s
        .get_link(quark::tenant::DEFAULT_TENANT, 101)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(got.url, "https://replica-routed.example");
}

/// Construction test. `open` points both pools at one URL and
/// `open_with_replica(a, b)` builds two pools; both configurations must
/// round-trip a write-then-read. sqlx does not expose the inner pool pointer,
/// so (as the design permits) this asserts behaviorally that each constructor
/// wires functioning read and write pools rather than comparing handle
/// identity.
#[tokio::test]
#[file_serial]
async fn open_and_open_with_replica_both_wire_working_pools() {
    let Some(url) = std::env::var("QUARK_TEST_DATABASE_URL").ok() else {
        eprintln!("skip: QUARK_TEST_DATABASE_URL not set");
        return;
    };
    // Single URL: both pools are the same handle; the round-trip works.
    let single = PostgresStore::open(&url, false).await.unwrap();
    single.reset_for_tests().await.unwrap();
    single
        .put_link(
            quark::tenant::DEFAULT_TENANT,
            201,
            &plain_rec("https://single.example"),
        )
        .await
        .unwrap();
    assert_eq!(
        single
            .get_link(quark::tenant::DEFAULT_TENANT, 201)
            .await
            .unwrap()
            .unwrap()
            .url,
        "https://single.example"
    );

    // Distinct constructor: write pool and read pool built separately, both
    // against the same DB here; the round-trip still works.
    let split = PostgresStore::open_with_replica(&url, &url, false)
        .await
        .unwrap();
    split.reset_for_tests().await.unwrap();
    split
        .put_link(
            quark::tenant::DEFAULT_TENANT,
            202,
            &plain_rec("https://split.example"),
        )
        .await
        .unwrap();
    assert_eq!(
        split
            .get_link(quark::tenant::DEFAULT_TENANT, 202)
            .await
            .unwrap()
            .unwrap()
            .url,
        "https://split.example"
    );
}

#[tokio::test]
#[file_serial]
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
        folder: None,
        fallback_url: None,
        password_hash: None,
        tenant_id: quark::tenant::DEFAULT_TENANT,
    };
    s.put_link(quark::tenant::DEFAULT_TENANT, 7, &rec)
        .await
        .unwrap();
    let got = s
        .get_link(quark::tenant::DEFAULT_TENANT, 7)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(got.url, "https://example.com");
    assert_eq!(got.app_ios.as_deref(), Some("https://apps.apple.com/x"));
    assert_eq!(got.app_android, None);
    assert!(s
        .get_link(quark::tenant::DEFAULT_TENANT, 999)
        .await
        .unwrap()
        .is_none());
}

#[tokio::test]
#[file_serial]
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
        folder: None,
        fallback_url: None,
        password_hash: None,
        tenant_id: quark::tenant::DEFAULT_TENANT,
    };
    s.put_link(quark::tenant::DEFAULT_TENANT, 42, &rec)
        .await
        .unwrap();
    let got = s
        .get_link(quark::tenant::DEFAULT_TENANT, 42)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(got.rules, rec.rules);
}

#[tokio::test]
#[file_serial]
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
        folder: None,
        fallback_url: None,
        password_hash: None,
        tenant_id: quark::tenant::DEFAULT_TENANT,
    };
    s.put_link(quark::tenant::DEFAULT_TENANT, 43, &rec)
        .await
        .unwrap();
    let got = s
        .get_link(quark::tenant::DEFAULT_TENANT, 43)
        .await
        .unwrap()
        .unwrap();
    assert!(got.rules.is_empty());
}

#[tokio::test]
#[file_serial]
async fn next_id_increments_pg() {
    let Some(s) = fresh().await else {
        return;
    };
    let a = s.next_id(quark::tenant::DEFAULT_TENANT).await.unwrap();
    let b = s.next_id(quark::tenant::DEFAULT_TENANT).await.unwrap();
    assert_eq!(b, a + 1);
}

#[tokio::test]
#[file_serial]
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
        folder: None,
        fallback_url: None,
        password_hash: None,
        tenant_id: quark::tenant::DEFAULT_TENANT,
    };
    assert!(s
        .put_alias_and_link(
            quark::tenant::DEFAULT_TENANT,
            quark::domain::SHARED_DOMAIN_ID,
            "promo",
            5,
            &rec
        )
        .await
        .unwrap());
    assert!(!s
        .put_alias_and_link(
            quark::tenant::DEFAULT_TENANT,
            quark::domain::SHARED_DOMAIN_ID,
            "promo",
            9,
            &rec
        )
        .await
        .unwrap());
    assert_eq!(
        s.get_alias(quark::domain::SHARED_DOMAIN_ID, "promo")
            .await
            .unwrap(),
        Some(5)
    );
    assert!(s
        .get_link(quark::tenant::DEFAULT_TENANT, 9)
        .await
        .unwrap()
        .is_none());
}

#[tokio::test]
#[file_serial]
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
        folder: None,
        fallback_url: None,
        password_hash: None,
        tenant_id: quark::tenant::DEFAULT_TENANT,
    };
    s.put_link(
        quark::tenant::DEFAULT_TENANT,
        1,
        &rec("https://a.com", &["rust", "web"]),
    )
    .await
    .unwrap();
    s.put_link(
        quark::tenant::DEFAULT_TENANT,
        2,
        &rec("https://b.com", &["web"]),
    )
    .await
    .unwrap();
    s.put_link(quark::tenant::DEFAULT_TENANT, 3, &rec("https://c.com", &[]))
        .await
        .unwrap();

    let got = s
        .get_link(quark::tenant::DEFAULT_TENANT, 1)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(got.tags, vec!["rust".to_string(), "web".to_string()]);

    let filtered = s
        .list_links(
            quark::tenant::DEFAULT_TENANT,
            None,
            50,
            Some("rust"),
            None,
            false,
        )
        .await
        .unwrap();
    assert_eq!(
        filtered.iter().map(|(id, _)| *id).collect::<Vec<_>>(),
        vec![1]
    );

    let both_web = s
        .list_links(
            quark::tenant::DEFAULT_TENANT,
            None,
            50,
            Some("web"),
            None,
            false,
        )
        .await
        .unwrap();
    let mut ids: Vec<u64> = both_web.iter().map(|(id, _)| *id).collect();
    ids.sort();
    assert_eq!(ids, vec![1, 2]);

    let tags = s.list_tags(quark::tenant::DEFAULT_TENANT).await.unwrap();
    assert_eq!(tags, vec![("rust".to_string(), 1), ("web".to_string(), 2)]);
}

#[tokio::test]
#[file_serial]
async fn folder_round_trip_filter_and_list_pg() {
    let Some(s) = fresh().await else {
        return;
    };
    let rec = |u: &str, folder: Option<&str>| Record {
        url: u.into(),
        expiry: None,
        created: 0,
        tags: Vec::new(),
        max_visits: None,
        rules: Vec::new(),
        variants: Vec::new(),
        app_ios: None,
        app_android: None,
        folder: folder.map(str::to_string),
        fallback_url: None,
        password_hash: None,
        tenant_id: quark::tenant::DEFAULT_TENANT,
    };
    s.put_link(
        quark::tenant::DEFAULT_TENANT,
        1,
        &rec("https://a.com", Some("Marketing")),
    )
    .await
    .unwrap();
    s.put_link(
        quark::tenant::DEFAULT_TENANT,
        2,
        &rec("https://b.com", Some("Marketing")),
    )
    .await
    .unwrap();
    s.put_link(
        quark::tenant::DEFAULT_TENANT,
        3,
        &rec("https://c.com", Some("Docs")),
    )
    .await
    .unwrap();
    s.put_link(
        quark::tenant::DEFAULT_TENANT,
        4,
        &rec("https://d.com", None),
    )
    .await
    .unwrap();

    let got = s
        .get_link(quark::tenant::DEFAULT_TENANT, 1)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(got.folder.as_deref(), Some("Marketing"));

    let filtered = s
        .list_links(
            quark::tenant::DEFAULT_TENANT,
            None,
            50,
            None,
            Some("marketing"),
            false,
        )
        .await
        .unwrap();
    let mut ids: Vec<u64> = filtered.iter().map(|(id, _)| *id).collect();
    ids.sort();
    assert_eq!(ids, vec![1, 2]);

    let folders = s.list_folders(quark::tenant::DEFAULT_TENANT).await.unwrap();
    assert_eq!(
        folders,
        vec![("Docs".to_string(), 1u64), ("Marketing".to_string(), 2u64)]
    );
}

#[tokio::test]
#[file_serial]
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
        folder: None,
        fallback_url: None,
        password_hash: None,
        tenant_id: quark::tenant::DEFAULT_TENANT,
    };
    s.put_link(quark::tenant::DEFAULT_TENANT, 11, &rec)
        .await
        .unwrap();
    assert_eq!(
        s.visits(quark::tenant::DEFAULT_TENANT, 11).await.unwrap(),
        0
    );
    assert_eq!(
        s.get_link(quark::tenant::DEFAULT_TENANT, 11)
            .await
            .unwrap()
            .unwrap()
            .max_visits,
        Some(5)
    );
}

#[tokio::test]
#[file_serial]
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
        folder: None,
        fallback_url: None,
        password_hash: None,
        tenant_id: quark::tenant::DEFAULT_TENANT,
    };
    s.put_link(quark::tenant::DEFAULT_TENANT, 12, &rec)
        .await
        .unwrap();
    assert_eq!(
        s.bump_visits(quark::tenant::DEFAULT_TENANT, 12)
            .await
            .unwrap(),
        1
    );
    assert_eq!(
        s.bump_visits(quark::tenant::DEFAULT_TENANT, 12)
            .await
            .unwrap(),
        2
    );
    assert_eq!(
        s.visits(quark::tenant::DEFAULT_TENANT, 12).await.unwrap(),
        2
    );

    let s = std::sync::Arc::new(s);
    let mut handles = Vec::new();
    for _ in 0..10 {
        let s2 = s.clone();
        handles.push(tokio::spawn(async move {
            s2.bump_visits(quark::tenant::DEFAULT_TENANT, 12)
                .await
                .unwrap()
        }));
    }
    let mut results = Vec::new();
    for h in handles {
        results.push(h.await.unwrap());
    }
    results.sort();
    assert_eq!(results, (3..=12).collect::<Vec<u64>>());
    assert_eq!(
        s.visits(quark::tenant::DEFAULT_TENANT, 12).await.unwrap(),
        12
    );
}

#[tokio::test]
#[file_serial]
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
        folder: None,
        fallback_url: None,
        password_hash: None,
        tenant_id: quark::tenant::DEFAULT_TENANT,
    };
    s.put_link(quark::tenant::DEFAULT_TENANT, 11, &rec)
        .await
        .unwrap();
    let got = s
        .get_link(quark::tenant::DEFAULT_TENANT, 11)
        .await
        .unwrap()
        .unwrap();
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
        folder: None,
        fallback_url: None,
        password_hash: None,
        tenant_id: quark::tenant::DEFAULT_TENANT,
    };
    s.put_link(quark::tenant::DEFAULT_TENANT, 12, &plain)
        .await
        .unwrap();
    assert!(s
        .get_link(quark::tenant::DEFAULT_TENANT, 12)
        .await
        .unwrap()
        .unwrap()
        .variants
        .is_empty());
}

#[tokio::test]
#[file_serial]
async fn wellknown_round_trip_pg() {
    let Some(s) = fresh().await else {
        return;
    };
    assert_eq!(
        s.get_wellknown(quark::tenant::DEFAULT_TENANT, "assetlinks.json")
            .await
            .unwrap(),
        None
    );
    let body = r#"{"relation":["delegate_permission/common.handle_all_urls"]}"#;
    s.put_wellknown(quark::tenant::DEFAULT_TENANT, "assetlinks.json", body)
        .await
        .unwrap();
    assert_eq!(
        s.get_wellknown(quark::tenant::DEFAULT_TENANT, "assetlinks.json")
            .await
            .unwrap(),
        Some(body.to_string())
    );
    s.delete_wellknown(quark::tenant::DEFAULT_TENANT, "assetlinks.json")
        .await
        .unwrap();
    assert_eq!(
        s.get_wellknown(quark::tenant::DEFAULT_TENANT, "assetlinks.json")
            .await
            .unwrap(),
        None
    );
    s.delete_wellknown(quark::tenant::DEFAULT_TENANT, "assetlinks.json")
        .await
        .unwrap();
}

/// `put_link_tx` writes BOTH the link and the delivery rows in one transaction:
/// after the call, the link and its `webhook_deliveries` row are present.
#[tokio::test]
#[file_serial]
async fn put_link_tx_writes_link_and_deliveries_atomically() {
    let Some((s, pool)) = fresh_with_pool().await else {
        eprintln!("skip: QUARK_TEST_DATABASE_URL not set");
        return;
    };
    let sub = add_sub(&s, "https://e.com/hook").await;
    let key = format!("evt_test.{}", sub.id);
    let rec = plain_rec("https://example.com");
    s.put_link_tx(
        quark::tenant::DEFAULT_TENANT,
        42,
        &rec,
        &[outbox_row(&key, sub.id, quark::now())],
    )
    .await
    .unwrap();

    assert_eq!(
        s.get_link(quark::tenant::DEFAULT_TENANT, 42)
            .await
            .unwrap()
            .unwrap()
            .url,
        "https://example.com"
    );
    assert_eq!(count_deliveries(&pool, &key).await, 1);
}

/// `put_link_tx` with no deliveries still upserts the link (patch on a link
/// with no matching subscription).
#[tokio::test]
#[file_serial]
async fn put_link_tx_with_no_deliveries_upserts_link() {
    let Some((s, pool)) = fresh_with_pool().await else {
        return;
    };
    let rec = plain_rec("https://only-link.com");
    s.put_link_tx(quark::tenant::DEFAULT_TENANT, 43, &rec, &[])
        .await
        .unwrap();
    assert_eq!(
        s.get_link(quark::tenant::DEFAULT_TENANT, 43)
            .await
            .unwrap()
            .unwrap()
            .url,
        "https://only-link.com"
    );
    assert_eq!(count_deliveries(&pool, "none").await, 0);
}

/// `put_alias_and_link_tx` writes the alias, the link, and the deliveries in
/// one transaction.
#[tokio::test]
#[file_serial]
async fn put_alias_and_link_tx_writes_all_atomically() {
    let Some((s, pool)) = fresh_with_pool().await else {
        return;
    };
    let sub = add_sub(&s, "https://e.com/hook").await;
    let key = format!("evt_test.{}", sub.id);
    let rec = plain_rec("https://aliased.com");
    let claimed = s
        .put_alias_and_link_tx(
            quark::tenant::DEFAULT_TENANT,
            quark::domain::SHARED_DOMAIN_ID,
            "promo",
            5,
            &rec,
            &[outbox_row(&key, sub.id, quark::now())],
        )
        .await
        .unwrap();
    assert!(claimed);
    assert_eq!(
        s.get_alias(quark::domain::SHARED_DOMAIN_ID, "promo")
            .await
            .unwrap(),
        Some(5)
    );
    assert_eq!(
        s.get_link(quark::tenant::DEFAULT_TENANT, 5)
            .await
            .unwrap()
            .unwrap()
            .url,
        "https://aliased.com"
    );
    assert_eq!(count_deliveries(&pool, &key).await, 1);
}

/// On an alias conflict, `put_alias_and_link_tx` returns `Ok(false)` and rolls
/// back: NEITHER the link NOR the deliveries are written.
#[tokio::test]
#[file_serial]
async fn put_alias_and_link_tx_conflict_rolls_back_link_and_deliveries() {
    let Some((s, pool)) = fresh_with_pool().await else {
        return;
    };
    let sub = add_sub(&s, "https://e.com/hook").await;
    // Claim the alias first with a different id.
    assert!(s
        .put_alias_and_link_tx(
            quark::tenant::DEFAULT_TENANT,
            quark::domain::SHARED_DOMAIN_ID,
            "promo",
            5,
            &plain_rec("https://first.com"),
            &[]
        )
        .await
        .unwrap());

    let key = format!("evt_test.{}", sub.id);
    let claimed = s
        .put_alias_and_link_tx(
            quark::tenant::DEFAULT_TENANT,
            quark::domain::SHARED_DOMAIN_ID,
            "promo",
            9,
            &plain_rec("https://second.com"),
            &[outbox_row(&key, sub.id, quark::now())],
        )
        .await
        .unwrap();
    assert!(!claimed, "alias already in use");
    // The losing link (id 9) must not exist, and its delivery must not be enqueued.
    assert!(s
        .get_link(quark::tenant::DEFAULT_TENANT, 9)
        .await
        .unwrap()
        .is_none());
    assert_eq!(
        count_deliveries(&pool, &key).await,
        0,
        "deliveries rolled back with the mutation"
    );
    // The original alias still points at id 5.
    assert_eq!(
        s.get_alias(quark::domain::SHARED_DOMAIN_ID, "promo")
            .await
            .unwrap(),
        Some(5)
    );
}

/// `delete_link_tx` deletes the link AND enqueues the deliveries atomically.
#[tokio::test]
#[file_serial]
async fn delete_link_tx_deletes_link_and_enqueues_deliveries() {
    let Some((s, pool)) = fresh_with_pool().await else {
        return;
    };
    let sub = add_sub(&s, "https://e.com/hook").await;
    s.put_link(
        quark::tenant::DEFAULT_TENANT,
        77,
        &plain_rec("https://doomed.com"),
    )
    .await
    .unwrap();
    let key = format!("evt_test.{}", sub.id);
    s.delete_link_tx(
        quark::tenant::DEFAULT_TENANT,
        77,
        &[outbox_row(&key, sub.id, quark::now())],
    )
    .await
    .unwrap();
    assert!(s
        .get_link(quark::tenant::DEFAULT_TENANT, 77)
        .await
        .unwrap()
        .is_none());
    assert_eq!(count_deliveries(&pool, &key).await, 1);
}

/// `ON CONFLICT (delivery_key) DO NOTHING` inside the tx: re-enqueuing an
/// existing `delivery_key` still upserts the link and leaves exactly one
/// delivery row.
#[tokio::test]
#[file_serial]
async fn put_link_tx_on_conflict_upserts_link_and_keeps_one_delivery() {
    let Some((s, pool)) = fresh_with_pool().await else {
        return;
    };
    let sub = add_sub(&s, "https://e.com/hook").await;
    let key = format!("evt_test.{}", sub.id);
    let at = quark::now();
    s.put_link_tx(
        quark::tenant::DEFAULT_TENANT,
        50,
        &plain_rec("https://v1.com"),
        &[outbox_row(&key, sub.id, at)],
    )
    .await
    .unwrap();
    // Second call reuses the same delivery_key but a new link body.
    s.put_link_tx(
        quark::tenant::DEFAULT_TENANT,
        50,
        &plain_rec("https://v2.com"),
        &[outbox_row(&key, sub.id, at)],
    )
    .await
    .unwrap();
    assert_eq!(
        s.get_link(quark::tenant::DEFAULT_TENANT, 50)
            .await
            .unwrap()
            .unwrap()
            .url,
        "https://v2.com"
    );
    assert_eq!(
        count_deliveries(&pool, &key).await,
        1,
        "duplicate delivery_key inserts one row"
    );
}

#[tokio::test]
#[file_serial]
async fn link_health_round_trip_pg() {
    let Some(s) = fresh().await else { return };
    assert!(s
        .list_link_health(quark::tenant::DEFAULT_TENANT)
        .await
        .unwrap()
        .is_empty());

    s.put_link_health(
        quark::tenant::DEFAULT_TENANT,
        1,
        &quark::store::LinkHealth {
            checked_at: 100,
            status: Some(200),
            healthy: true,
        },
    )
    .await
    .unwrap();
    s.put_link_health(
        quark::tenant::DEFAULT_TENANT,
        2,
        &quark::store::LinkHealth {
            checked_at: 100,
            status: None,
            healthy: false,
        },
    )
    .await
    .unwrap();
    // Overwrite id 1: healthy -> broken.
    s.put_link_health(
        quark::tenant::DEFAULT_TENANT,
        1,
        &quark::store::LinkHealth {
            checked_at: 200,
            status: Some(500),
            healthy: false,
        },
    )
    .await
    .unwrap();

    let map: std::collections::HashMap<u64, quark::store::LinkHealth> = s
        .list_link_health(quark::tenant::DEFAULT_TENANT)
        .await
        .unwrap()
        .into_iter()
        .collect();
    assert_eq!(map.len(), 2);
    assert_eq!(
        map[&1],
        quark::store::LinkHealth {
            checked_at: 200,
            status: Some(500),
            healthy: false
        }
    );
    assert_eq!(map[&2].status, None);
    assert!(!map[&2].healthy);
}

#[tokio::test]
#[file_serial]
async fn health_lease_single_holder_and_renew_pg() {
    let Some(s) = fresh().await else { return };
    // First holder acquires; a different holder is refused while it is valid;
    // the holder can renew.
    assert!(s.try_acquire_health_lease("node-a", 60).await.unwrap());
    assert!(!s.try_acquire_health_lease("node-b", 60).await.unwrap());
    assert!(s.try_acquire_health_lease("node-a", 60).await.unwrap());
}

#[tokio::test]
#[file_serial]
async fn list_broken_link_ids_pg() {
    let Some(s) = fresh().await else { return };
    s.put_link_health(
        quark::tenant::DEFAULT_TENANT,
        3,
        &quark::store::LinkHealth {
            checked_at: 1,
            status: Some(200),
            healthy: true,
        },
    )
    .await
    .unwrap();
    s.put_link_health(
        quark::tenant::DEFAULT_TENANT,
        1,
        &quark::store::LinkHealth {
            checked_at: 1,
            status: Some(404),
            healthy: false,
        },
    )
    .await
    .unwrap();
    s.put_link_health(
        quark::tenant::DEFAULT_TENANT,
        2,
        &quark::store::LinkHealth {
            checked_at: 1,
            status: None,
            healthy: false,
        },
    )
    .await
    .unwrap();
    assert_eq!(
        s.list_broken_link_ids(quark::tenant::DEFAULT_TENANT)
            .await
            .unwrap(),
        vec![1, 2]
    );
}

#[tokio::test]
#[file_serial]
async fn sheets_connection_round_trips_pg() {
    let Some(s) = fresh().await else { return };
    assert!(s
        .get_sheets_connection(quark::tenant::DEFAULT_TENANT)
        .await
        .unwrap()
        .is_none());
    let c = quark::sheets::SheetsConnection {
        refresh_token: "rt".into(),
        email: "me@x.com".into(),
        spreadsheet_id: Some("s1".into()),
        last_sync: Some(5),
        last_status: quark::sheets::SyncStatus::Ok,
    };
    s.put_sheets_connection(quark::tenant::DEFAULT_TENANT, &c)
        .await
        .unwrap();
    let got = s
        .get_sheets_connection(quark::tenant::DEFAULT_TENANT)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(got.email, "me@x.com");
    assert_eq!(got.spreadsheet_id.as_deref(), Some("s1"));
    // Upsert replaces the single row.
    let c2 = quark::sheets::SheetsConnection {
        email: "other@x.com".into(),
        ..c.clone()
    };
    s.put_sheets_connection(quark::tenant::DEFAULT_TENANT, &c2)
        .await
        .unwrap();
    let got = s
        .get_sheets_connection(quark::tenant::DEFAULT_TENANT)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(got.email, "other@x.com");
    s.delete_sheets_connection(quark::tenant::DEFAULT_TENANT)
        .await
        .unwrap();
    assert!(s
        .get_sheets_connection(quark::tenant::DEFAULT_TENANT)
        .await
        .unwrap()
        .is_none());
}

#[tokio::test]
#[file_serial]
async fn sheets_lease_single_holder_and_renew_pg() {
    let Some(s) = fresh().await else { return };
    // Mirrors the health lease: first holder acquires; a different holder is
    // refused while it is valid; the holder can renew.
    assert!(s.try_acquire_sheets_lease("node-a", 60).await.unwrap());
    assert!(!s.try_acquire_sheets_lease("node-b", 60).await.unwrap());
    assert!(s.try_acquire_sheets_lease("node-a", 60).await.unwrap());
}

#[tokio::test]
#[file_serial]
async fn session_round_trip_and_gc_pg() {
    let Some(s) = fresh().await else { return };
    let sess = quark::auth::Session {
        token_hash: "h1".into(),
        subject: "sub-1".into(),
        display: "a@example.com".into(),
        scopes: vec![quark::auth::Scope::LinksRead, quark::auth::Scope::Analytics],
        created: 10,
        expires: 100,
        tenant_id: quark::tenant::DEFAULT_TENANT,
        user_id: 0,
        id_token: None,
    };
    s.put_session(quark::tenant::DEFAULT_TENANT, &sess)
        .await
        .unwrap();
    let got = s.get_session_by_hash("h1", 50).await.unwrap().unwrap();
    assert_eq!(got.subject, "sub-1");
    assert_eq!(
        got.scopes,
        vec![quark::auth::Scope::LinksRead, quark::auth::Scope::Analytics]
    );
    // Expired is not returned.
    assert!(s.get_session_by_hash("h1", 100).await.unwrap().is_none());
    // gc drops expired rows only.
    s.put_session(
        quark::tenant::DEFAULT_TENANT,
        &quark::auth::Session {
            token_hash: "old".into(),
            expires: 5,
            ..sess.clone()
        },
    )
    .await
    .unwrap();
    s.gc_sessions(50).await.unwrap();
    assert!(s.get_session_by_hash("old", 4).await.unwrap().is_none());
    assert!(s.get_session_by_hash("h1", 50).await.unwrap().is_some());
    // delete (logout).
    s.delete_session("h1").await.unwrap();
    assert!(s.get_session_by_hash("h1", 50).await.unwrap().is_none());
}
