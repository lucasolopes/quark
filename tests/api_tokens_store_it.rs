use quark::auth::{hash_token, ApiToken, Scope};
use quark::store::{postgres::PostgresStore, Store};
use serial_test::serial;

async fn fresh() -> Option<PostgresStore> {
    let url = std::env::var("QUARK_TEST_DATABASE_URL").ok()?;
    let s = PostgresStore::open(&url, false).await.unwrap();
    s.reset_for_tests().await.unwrap();
    Some(s)
}

#[tokio::test]
#[serial(pg)]
async fn api_token_crud_round_trip_pg() {
    let Some(store) = fresh().await else {
        eprintln!("skip: QUARK_TEST_DATABASE_URL not set");
        return;
    };
    let id = store
        .next_api_token_id(quark::tenant::DEFAULT_TENANT)
        .await
        .unwrap();
    let hash = hash_token("qtok_abc123");
    let token = ApiToken {
        id,
        name: "ci".into(),
        token_hash: hash.clone(),
        scopes: vec![Scope::LinksRead, Scope::Webhooks],
        rate_limit_per_min: Some(60),
        created: 1,
        tenant_id: quark::tenant::DEFAULT_TENANT,
    };
    store
        .put_api_token(quark::tenant::DEFAULT_TENANT, &token)
        .await
        .unwrap();

    assert_eq!(
        store.get_api_token_by_hash(&hash).await.unwrap(),
        Some(token)
    );
    assert_eq!(
        store
            .list_api_tokens(quark::tenant::DEFAULT_TENANT)
            .await
            .unwrap()
            .len(),
        1
    );
    assert!(store
        .delete_api_token(quark::tenant::DEFAULT_TENANT, id)
        .await
        .unwrap());
    assert_eq!(store.get_api_token_by_hash(&hash).await.unwrap(), None);
}

#[tokio::test]
#[serial(pg)]
async fn delete_api_token_returns_false_when_missing_pg() {
    let Some(store) = fresh().await else {
        return;
    };
    assert!(!store
        .delete_api_token(quark::tenant::DEFAULT_TENANT, 999)
        .await
        .unwrap());
}

#[tokio::test]
#[serial(pg)]
async fn next_api_token_id_increments_pg() {
    let Some(store) = fresh().await else {
        return;
    };
    let a = store
        .next_api_token_id(quark::tenant::DEFAULT_TENANT)
        .await
        .unwrap();
    let b = store
        .next_api_token_id(quark::tenant::DEFAULT_TENANT)
        .await
        .unwrap();
    assert_eq!(b, a + 1);
}
