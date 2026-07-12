use quark::store::{Record, Store};

fn tmp() -> tempfile::TempDir {
    tempfile::tempdir().unwrap()
}

#[test]
fn put_get_link() {
    let dir = tmp();
    let store = Store::open(dir.path()).unwrap();
    let rec = Record { url: "https://example.com".into(), expiry: None, created: 100 };
    store.put_link(7, &rec).unwrap();
    let got = store.get_link(7).unwrap().unwrap();
    assert_eq!(got.url, "https://example.com");
    assert!(store.get_link(999).unwrap().is_none());
}

#[test]
fn next_id_incrementa_e_persiste() {
    let dir = tmp();
    {
        let store = Store::open(dir.path()).unwrap();
        assert_eq!(store.next_id().unwrap(), 1);
        assert_eq!(store.next_id().unwrap(), 2);
    }
    // reabrir: contador persiste
    let store = Store::open(dir.path()).unwrap();
    assert_eq!(store.next_id().unwrap(), 3);
}

#[test]
fn alias_nao_sobrescreve() {
    let dir = tmp();
    let store = Store::open(dir.path()).unwrap();
    assert!(store.put_alias("promo", 5).unwrap());
    assert!(!store.put_alias("promo", 9).unwrap());
    assert_eq!(store.get_alias("promo").unwrap(), Some(5));
    assert_eq!(store.get_alias("inexistente").unwrap(), None);
}
