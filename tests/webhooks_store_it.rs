// Codigo de teste pode entrar em panico: a falha e o proprio sinal. O
// clippy.toml cobre itens sob #[test]/#[cfg(test)], mas nao os helpers de
// topo de arquivo (fn app(), fixtures), que sao a maioria aqui.
#![allow(clippy::unwrap_used)]

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
        disabled_reason: None,
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
            .url
            .expose(),
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
async fn record_webhook_health_updates_only_health_fields_pg() {
    let Some(store) = fresh().await else {
        eprintln!("skip: QUARK_TEST_DATABASE_URL not set");
        return;
    };
    let sub = WebhookSubscription {
        id: 1,
        url: "https://h/x".into(),
        events: vec![EventType::LinkCreated],
        secret: String::new(),
        active: true,
        created: 10,
        kind: SubscriptionKind::Generic,
        label: None,
        connector_id: Some("zapier".into()),
        external_id: None,
        last_delivery_at: None,
        last_delivery_status: quark::health::HealthStatus::Never,
        disabled_reason: None,
    };
    store
        .put_webhook(quark::tenant::DEFAULT_TENANT, &sub)
        .await
        .unwrap();

    store
        .record_webhook_health(
            quark::tenant::DEFAULT_TENANT,
            1,
            200,
            quark::health::HealthStatus::Error("502".into()),
        )
        .await
        .unwrap();

    let got = store
        .get_webhook(quark::tenant::DEFAULT_TENANT, 1)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(got.last_delivery_at, Some(200));
    assert_eq!(
        got.last_delivery_status,
        quark::health::HealthStatus::Error("502".into())
    );
    // Campos nao-health preservados.
    assert_eq!(got.connector_id.as_deref(), Some("zapier"));
    assert_eq!(got.url.expose(), "https://h/x");
    assert!(got.active);
}

/// Uma subscription ativa apontando para `url`, pronta para `put_webhook`.
fn active_sub(id: u64, url: &str) -> WebhookSubscription {
    WebhookSubscription {
        id,
        url: url.into(),
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
        disabled_reason: None,
    }
}

#[tokio::test]
#[file_serial]
async fn disable_webhook_sets_inactive_with_reason_pg() {
    let Some(store) = fresh().await else {
        eprintln!("skip: QUARK_TEST_DATABASE_URL not set");
        return;
    };
    let tenant = quark::tenant::DEFAULT_TENANT;
    let id = store.next_webhook_id(tenant).await.unwrap();
    store
        .put_webhook(tenant, &active_sub(id, "https://example.com/hook"))
        .await
        .unwrap();

    store
        .disable_webhook(tenant, id, "status 410")
        .await
        .unwrap();

    let got = store.get_webhook(tenant, id).await.unwrap().unwrap();
    assert!(!got.active, "a subscription deveria ter sido desativada");
    assert_eq!(got.disabled_reason.as_deref(), Some("status 410"));
}

#[tokio::test]
#[file_serial]
async fn reactivating_clears_the_disabled_reason_pg() {
    let Some(store) = fresh().await else {
        eprintln!("skip: QUARK_TEST_DATABASE_URL not set");
        return;
    };
    let tenant = quark::tenant::DEFAULT_TENANT;
    let id = store.next_webhook_id(tenant).await.unwrap();
    let mut sub = active_sub(id, "https://example.com/hook");
    store.put_webhook(tenant, &sub).await.unwrap();
    store
        .disable_webhook(tenant, id, "status 404")
        .await
        .unwrap();

    sub.active = true;
    sub.disabled_reason = None;
    store.put_webhook(tenant, &sub).await.unwrap();

    let got = store.get_webhook(tenant, id).await.unwrap().unwrap();
    assert!(got.active);
    assert_eq!(
        got.disabled_reason, None,
        "reativar tem que limpar o motivo"
    );
}

#[tokio::test]
#[file_serial]
async fn next_webhook_id_increments_pg() {
    let Some(store) = fresh().await else {
        eprintln!("skip: QUARK_TEST_DATABASE_URL not set");
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
