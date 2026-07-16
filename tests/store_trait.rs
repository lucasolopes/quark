use quark::store::{open_store, Record, Store};
use std::sync::Arc;

#[tokio::test]
async fn round_trip_via_trait_object() {
    let dir = tempfile::tempdir().unwrap();
    let store: Arc<dyn Store> = open_store(dir.path()).await.unwrap();

    let id = store.next_id(quark::tenant::DEFAULT_TENANT).await.unwrap();
    let rec = Record {
        url: "https://example.com/dyn".into(),
        expiry: None,
        created: 1,
        tags: Vec::new(),
        max_visits: None,
        rules: Vec::new(),
        variants: Vec::new(),
        app_ios: None,
        app_android: None,
        folder: None,
        fallback_url: None,
        password_hash: None,
    };
    store
        .put_link(quark::tenant::DEFAULT_TENANT, id, &rec)
        .await
        .unwrap();

    let got = store
        .get_link(quark::tenant::DEFAULT_TENANT, id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(got.url, "https://example.com/dyn");

    assert!(store
        .put_alias_and_link(quark::tenant::DEFAULT_TENANT, "promo-dyn", 999, &rec)
        .await
        .unwrap());
    assert_eq!(
        store
            .get_alias(quark::tenant::DEFAULT_TENANT, "promo-dyn")
            .await
            .unwrap(),
        Some(999)
    );
}
