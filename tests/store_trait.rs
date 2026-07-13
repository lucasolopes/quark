use quark::store::{open_store, Record, Store};
use std::sync::Arc;

#[tokio::test]
async fn round_trip_via_trait_object() {
    let dir = tempfile::tempdir().unwrap();
    // Exercita explicitamente o dispatch dinâmico: nada aqui conhece o LmdbStore.
    let store: Arc<dyn Store> = open_store(dir.path()).await.unwrap();

    let id = store.next_id().await.unwrap();
    let rec = Record {
        url: "https://example.com/dyn".into(),
        expiry: None,
        created: 1,
    };
    store.put_link(id, &rec).await.unwrap();

    let got = store.get_link(id).await.unwrap().unwrap();
    assert_eq!(got.url, "https://example.com/dyn");

    // alias transacional também via trait object
    assert!(store
        .put_alias_and_link("promo-dyn", 999, &rec)
        .await
        .unwrap());
    assert_eq!(store.get_alias("promo-dyn").await.unwrap(), Some(999));
}
