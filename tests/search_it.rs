use quark::store::{postgres::PostgresStore, Record, Store};
use serial_test::serial;

async fn fresh() -> Option<PostgresStore> {
    let url = std::env::var("QUARK_TEST_DATABASE_URL").ok()?;
    let s = PostgresStore::open(&url).await.unwrap();
    // limpa estado entre rodadas
    s.reset_for_tests().await.unwrap();
    Some(s)
}

/// Semeia links via `put_link`/`put_alias_and_link`. Retorna os ids alocados
/// na mesma ordem de `links`. `alias` (quando `Some`) fica associado ao id.
async fn seed_links(s: &PostgresStore, links: &[(&str, Option<&str>)]) -> Vec<u64> {
    let mut ids = Vec::new();
    for (url, alias) in links {
        let id = s.next_id().await.unwrap();
        let rec = Record {
            url: url.to_string(),
            expiry: None,
            created: 0,
        };
        match alias {
            Some(a) => {
                assert!(s.put_alias_and_link(a, id, &rec).await.unwrap());
            }
            None => {
                s.put_link(id, &rec).await.unwrap();
            }
        }
        ids.push(id);
    }
    ids
}

// Semeia 3 links: github.com/rust-lang, example.com, docs.rs com alias "rust".
// Busca "rust" casa por url (rust-lang) E por alias (o link com alias "rust").
#[tokio::test]
#[serial(pg)]
async fn search_matches_url_and_alias() {
    let Some(store) = fresh().await else {
        eprintln!("skip: sem QUARK_TEST_DATABASE_URL");
        return;
    };
    let ids = seed_links(
        &store,
        &[
            ("https://github.com/rust-lang", None),
            ("https://example.com", None),
            ("https://docs.rs", Some("rust")), // alias "rust", url NÃO contém rust
        ],
    )
    .await;
    let hits = store.search_links("rust", None, 50).await.unwrap();
    let urls: Vec<&str> = hits.iter().map(|(_, r)| r.url.as_str()).collect();
    assert!(
        urls.iter().any(|u| u.contains("github.com/rust-lang")),
        "casa url"
    );
    assert!(
        hits.iter().any(|(id, _)| *id == ids[2]),
        "casa alias 'rust'"
    );
    assert!(!urls.contains(&"https://example.com"), "não casa example");
}

// Curinga literal: "%" no termo NÃO vira wildcard SQL.
#[tokio::test]
#[serial(pg)]
async fn search_escapes_wildcards() {
    let Some(store) = fresh().await else {
        return;
    };
    seed_links(
        &store,
        &[
            ("https://ex.com/50%off", None),
            ("https://ex.com/504", None),
        ],
    )
    .await;
    let hits = store.search_links("50%", None, 50).await.unwrap();
    let urls: Vec<&str> = hits.iter().map(|(_, r)| r.url.as_str()).collect();
    assert!(
        urls.iter().any(|u| u.contains("50%off")),
        "casa o literal 50%off"
    );
    assert!(
        !urls.iter().any(|u| u.contains("504")),
        "NÃO casa 504 (% não é wildcard)"
    );
}

// Keyset: after corta os ids <= after.
#[tokio::test]
#[serial(pg)]
async fn search_keyset_pagination() {
    let Some(store) = fresh().await else {
        return;
    };
    let ids = seed_links(
        &store,
        &[
            ("https://ex.com/alfa", None),
            ("https://ex.com/alfa2", None),
            ("https://ex.com/alfa3", None),
        ],
    )
    .await;
    let page1 = store.search_links("alfa", None, 2).await.unwrap();
    assert_eq!(page1.len(), 2);
    let after = page1.last().unwrap().0;
    let page2 = store.search_links("alfa", Some(after), 2).await.unwrap();
    assert!(
        page2.iter().all(|(id, _)| *id > after),
        "página 2 só tem id > after"
    );
    assert!(page2.iter().any(|(id, _)| *id == ids[2]));
}
