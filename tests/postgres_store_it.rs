use quark::store::{postgres::PostgresStore, Record, Store};

async fn fresh() -> Option<PostgresStore> {
    let url = std::env::var("QUARK_TEST_DATABASE_URL").ok()?;
    let s = PostgresStore::open(&url).await.unwrap();
    // limpa estado entre rodadas
    s.reset_for_tests().await.unwrap();
    Some(s)
}

#[tokio::test]
async fn put_get_link_pg() {
    let Some(s) = fresh().await else {
        eprintln!("skip: sem QUARK_TEST_DATABASE_URL");
        return;
    };
    let rec = Record {
        url: "https://example.com".into(),
        expiry: None,
        created: 100,
    };
    s.put_link(7, &rec).await.unwrap();
    assert_eq!(
        s.get_link(7).await.unwrap().unwrap().url,
        "https://example.com"
    );
    assert!(s.get_link(999).await.unwrap().is_none());
}

#[tokio::test]
async fn next_id_incrementa_pg() {
    let Some(s) = fresh().await else {
        return;
    };
    let a = s.next_id().await.unwrap();
    let b = s.next_id().await.unwrap();
    assert_eq!(b, a + 1);
}

#[tokio::test]
async fn alias_atomico_sem_orfao_pg() {
    let Some(s) = fresh().await else {
        return;
    };
    let rec = Record {
        url: "u".into(),
        expiry: None,
        created: 0,
    };
    assert!(s.put_alias_and_link("promo", 5, &rec).await.unwrap());
    assert!(!s.put_alias_and_link("promo", 9, &rec).await.unwrap()); // colisão
    assert_eq!(s.get_alias("promo").await.unwrap(), Some(5));
    assert!(s.get_link(9).await.unwrap().is_none()); // sem órfão
}
