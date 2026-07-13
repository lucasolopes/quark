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
    };
    store.put_link(7, &rec).await.unwrap();
    let got = store.get_link(7).await.unwrap().unwrap();
    assert_eq!(got.url, "https://example.com");
    assert!(store.get_link(999).await.unwrap().is_none());
}

#[tokio::test]
async fn next_id_incrementa_e_persiste() {
    let dir = tmp();
    {
        let store = open_store(dir.path()).await.unwrap();
        assert_eq!(store.next_id().await.unwrap(), 1);
        assert_eq!(store.next_id().await.unwrap(), 2);
    }
    // reabrir: contador persiste
    let store = open_store(dir.path()).await.unwrap();
    assert_eq!(store.next_id().await.unwrap(), 3);
}

#[tokio::test]
async fn alias_nao_sobrescreve() {
    let dir = tmp();
    let store = open_store(dir.path()).await.unwrap();
    assert!(store.put_alias("promo", 5).await.unwrap());
    assert!(!store.put_alias("promo", 9).await.unwrap());
    assert_eq!(store.get_alias("promo").await.unwrap(), Some(5));
    assert_eq!(store.get_alias("inexistente").await.unwrap(), None);
}

#[tokio::test]
async fn put_alias_and_link_atomico() {
    let dir = tmp();
    let store = open_store(dir.path()).await.unwrap();
    let rec = Record {
        url: "https://example.com".into(),
        expiry: None,
        created: 100,
    };
    let rec2 = Record {
        url: "https://outro.com".into(),
        expiry: None,
        created: 200,
    };

    assert!(store.put_alias_and_link("promo", 5, &rec).await.unwrap());
    assert_eq!(store.get_alias("promo").await.unwrap(), Some(5));
    assert_eq!(
        store.get_link(5).await.unwrap().unwrap().url,
        "https://example.com"
    );

    assert!(!store.put_alias_and_link("promo", 9, &rec2).await.unwrap());
    assert_eq!(store.get_alias("promo").await.unwrap(), Some(5));
    assert!(store.get_link(9).await.unwrap().is_none());
}
