use quark::store::{postgres::PostgresStore, Store};
use quark::webhooks::{EventType, SubscriptionKind, WebhookSubscription};
use serial_test::file_serial;

async fn fresh() -> Option<PostgresStore> {
    let url = std::env::var("QUARK_TEST_DATABASE_URL").ok()?;
    let s = PostgresStore::open(&url, false).await.unwrap();
    s.reset_for_tests().await.unwrap();
    Some(s)
}

#[tokio::test]
#[file_serial]
async fn webhook_crud_round_trip_pg() {
    let Some(store) = fresh().await else {
        eprintln!("skip: QUARK_TEST_DATABASE_URL not set");
        return;
    };
    let id = store
        .next_webhook_id(quark::tenant::DEFAULT_TENANT)
        .await
        .unwrap();
    let sub = WebhookSubscription {
        id,
        url: "https://e.com".into(),
        events: vec![EventType::LinkCreated],
        secret: "whsec_a".into(),
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
    assert_eq!(
        store
            .get_webhook(quark::tenant::DEFAULT_TENANT, id)
            .await
            .unwrap()
            .unwrap()
            .url,
        "https://e.com"
    );
    assert_eq!(
        store
            .list_webhooks(quark::tenant::DEFAULT_TENANT)
            .await
            .unwrap()
            .len(),
        1
    );
    assert!(store
        .delete_webhook(quark::tenant::DEFAULT_TENANT, id)
        .await
        .unwrap());
    assert!(store
        .get_webhook(quark::tenant::DEFAULT_TENANT, id)
        .await
        .unwrap()
        .is_none());
}

#[tokio::test]
#[file_serial]
async fn next_webhook_id_increments_pg() {
    let Some(store) = fresh().await else {
        return;
    };
    let a = store
        .next_webhook_id(quark::tenant::DEFAULT_TENANT)
        .await
        .unwrap();
    let b = store
        .next_webhook_id(quark::tenant::DEFAULT_TENANT)
        .await
        .unwrap();
    assert_eq!(b, a + 1);
}
