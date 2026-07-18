use quark::store::{postgres::PostgresStore, Record, Store};
use serial_test::serial;

async fn fresh() -> Option<PostgresStore> {
    let url = std::env::var("QUARK_TEST_DATABASE_URL").ok()?;
    let s = PostgresStore::open(&url, false).await.unwrap();
    s.reset_for_tests().await.unwrap();
    Some(s)
}

/// Seeds links via `put_link`/`put_alias_and_link`. Returns the allocated ids
/// in the same order as `links`. `alias` (when `Some`) is associated with the id.
async fn seed_links(s: &PostgresStore, links: &[(&str, Option<&str>)]) -> Vec<u64> {
    let mut ids = Vec::new();
    for (url, alias) in links {
        let id = s.next_id(quark::tenant::DEFAULT_TENANT).await.unwrap();
        let rec = Record {
            url: url.to_string(),
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
        match alias {
            Some(a) => {
                assert!(s
                    .put_alias_and_link(
                        quark::tenant::DEFAULT_TENANT,
                        quark::domain::SHARED_DOMAIN_ID,
                        a,
                        id,
                        &rec
                    )
                    .await
                    .unwrap());
            }
            None => {
                s.put_link(quark::tenant::DEFAULT_TENANT, id, &rec)
                    .await
                    .unwrap();
            }
        }
        ids.push(id);
    }
    ids
}

#[tokio::test]
#[serial(pg)]
async fn search_matches_url_and_alias() {
    let Some(store) = fresh().await else {
        eprintln!("skip: QUARK_TEST_DATABASE_URL not set");
        return;
    };
    let ids = seed_links(
        &store,
        &[
            ("https://github.com/rust-lang", None),
            ("https://example.com", None),
            ("https://docs.rs", Some("rust")), // alias "rust", url does NOT contain rust
        ],
    )
    .await;
    let hits = store
        .search_links(
            quark::tenant::DEFAULT_TENANT,
            "rust",
            None,
            50,
            None,
            None,
            false,
        )
        .await
        .unwrap();
    let urls: Vec<&str> = hits.iter().map(|(_, r)| r.url.as_str()).collect();
    assert!(
        urls.iter().any(|u| u.contains("github.com/rust-lang")),
        "matches url"
    );
    assert!(
        hits.iter().any(|(id, _)| *id == ids[2]),
        "matches alias 'rust'"
    );
    assert!(
        !urls.contains(&"https://example.com"),
        "does not match example"
    );
}

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
    let hits = store
        .search_links(
            quark::tenant::DEFAULT_TENANT,
            "50%",
            None,
            50,
            None,
            None,
            false,
        )
        .await
        .unwrap();
    let urls: Vec<&str> = hits.iter().map(|(_, r)| r.url.as_str()).collect();
    assert!(
        urls.iter().any(|u| u.contains("50%off")),
        "matches the literal 50%off"
    );
    assert!(
        !urls.iter().any(|u| u.contains("504")),
        "does NOT match 504 (% is not a wildcard)"
    );
}

#[tokio::test]
#[serial(pg)]
async fn search_is_case_insensitive() {
    let Some(store) = fresh().await else {
        return;
    };
    let ids = seed_links(
        &store,
        &[
            ("https://github.com/rust-lang", None),
            ("https://docs.rs", Some("rust")),
        ],
    )
    .await;
    let hits = store
        .search_links(
            quark::tenant::DEFAULT_TENANT,
            "RUST",
            None,
            50,
            None,
            None,
            false,
        )
        .await
        .unwrap();
    assert!(
        hits.iter()
            .any(|(_, r)| r.url.contains("github.com/rust-lang")),
        "matches url in different case"
    );
    assert!(
        hits.iter().any(|(id, _)| *id == ids[1]),
        "matches alias in different case"
    );
}

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
    let page1 = store
        .search_links(
            quark::tenant::DEFAULT_TENANT,
            "alfa",
            None,
            2,
            None,
            None,
            false,
        )
        .await
        .unwrap();
    assert_eq!(page1.len(), 2);
    let after = page1.last().unwrap().0;
    let page2 = store
        .search_links(
            quark::tenant::DEFAULT_TENANT,
            "alfa",
            Some(after),
            2,
            None,
            None,
            false,
        )
        .await
        .unwrap();
    assert!(
        page2.iter().all(|(id, _)| *id > after),
        "page 2 only has id > after"
    );
    assert!(page2.iter().any(|(id, _)| *id == ids[2]));
}
