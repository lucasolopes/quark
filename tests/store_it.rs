use quark::store::{open_store, Record};

fn tmp() -> tempfile::TempDir {
    tempfile::tempdir().unwrap()
}

#[tokio::test]
async fn put_get_link() {
    let dir = tmp();
    let store = open_store(dir.path()).await.unwrap();
    let rec = Record {
        url: "https://example.com".into(),
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
    };
    store
        .put_link(quark::tenant::DEFAULT_TENANT, 7, &rec)
        .await
        .unwrap();
    let got = store
        .get_link(quark::tenant::DEFAULT_TENANT, 7)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(got.url, "https://example.com");
    assert!(store
        .get_link(quark::tenant::DEFAULT_TENANT, 999)
        .await
        .unwrap()
        .is_none());
}

#[tokio::test]
async fn next_id_increments_and_persists() {
    let dir = tmp();
    {
        let store = open_store(dir.path()).await.unwrap();
        assert_eq!(
            store.next_id(quark::tenant::DEFAULT_TENANT).await.unwrap(),
            1
        );
        assert_eq!(
            store.next_id(quark::tenant::DEFAULT_TENANT).await.unwrap(),
            2
        );
    }
    let store = open_store(dir.path()).await.unwrap();
    assert_eq!(
        store.next_id(quark::tenant::DEFAULT_TENANT).await.unwrap(),
        3
    );
}

#[tokio::test]
async fn put_alias_and_link_is_atomic() {
    let dir = tmp();
    let store = open_store(dir.path()).await.unwrap();
    let rec = Record {
        url: "https://example.com".into(),
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
    };
    let rec2 = Record {
        url: "https://other.com".into(),
        expiry: None,
        created: 200,
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

    assert!(store
        .put_alias_and_link(
            quark::tenant::DEFAULT_TENANT,
            quark::domain::SHARED_DOMAIN_ID,
            "promo",
            5,
            &rec
        )
        .await
        .unwrap());
    assert_eq!(
        store
            .get_alias(quark::domain::SHARED_DOMAIN_ID, "promo")
            .await
            .unwrap(),
        Some(5)
    );
    assert_eq!(
        store
            .get_link(quark::tenant::DEFAULT_TENANT, 5)
            .await
            .unwrap()
            .unwrap()
            .url,
        "https://example.com"
    );

    assert!(!store
        .put_alias_and_link(
            quark::tenant::DEFAULT_TENANT,
            quark::domain::SHARED_DOMAIN_ID,
            "promo",
            9,
            &rec2
        )
        .await
        .unwrap());
    assert_eq!(
        store
            .get_alias(quark::domain::SHARED_DOMAIN_ID, "promo")
            .await
            .unwrap(),
        Some(5)
    );
    assert!(store
        .get_link(quark::tenant::DEFAULT_TENANT, 9)
        .await
        .unwrap()
        .is_none());
}

#[tokio::test]
async fn sheets_connection_round_trips() {
    let dir = tmp();
    let store = open_store(dir.path()).await.unwrap();
    assert!(store
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
    store
        .put_sheets_connection(quark::tenant::DEFAULT_TENANT, &c)
        .await
        .unwrap();
    let got = store
        .get_sheets_connection(quark::tenant::DEFAULT_TENANT)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(got.email, "me@x.com");
    assert_eq!(got.spreadsheet_id.as_deref(), Some("s1"));
    store
        .delete_sheets_connection(quark::tenant::DEFAULT_TENANT)
        .await
        .unwrap();
    assert!(store
        .get_sheets_connection(quark::tenant::DEFAULT_TENANT)
        .await
        .unwrap()
        .is_none());
}
