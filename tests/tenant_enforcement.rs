//! Multi-tenancy P2a enforcement: in cloud mode (`multi_tenant = true`) every
//! tenant-owned query runs inside a transaction that first did
//! `SET LOCAL app.tenant_id`, and the tenant-owned tables carry
//! `FORCE ROW LEVEL SECURITY`. That makes cross-tenant access fail closed at the
//! database, independently of the app-level `WHERE tenant_id` predicate (which
//! stays as belt-and-suspenders).
//!
//! Gated on `QUARK_TEST_DATABASE_URL` (needs a live Postgres). When it is unset
//! the test early-returns; the controller runs the gated arm against real
//! Postgres. Correct by construction either way.

use quark::analytics::{AnalyticsSink, ClickEvent};
use quark::store::postgres::PostgresStore;
use quark::store::{OutboxRow, Record, Store};
use quark::tenant::TenantId;
use quark::webhooks::{EventType, SubscriptionKind, WebhookSubscription};
use std::sync::Arc;

fn rec(url: &str) -> Record {
    Record {
        url: url.into(),
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
    }
}

#[tokio::test]
async fn cloud_force_rls_is_fail_closed() {
    let Some(url) = std::env::var("QUARK_TEST_DATABASE_URL").ok() else {
        eprintln!("skip: QUARK_TEST_DATABASE_URL not set");
        return;
    };
    // multi_tenant = true -> FORCE RLS + per-tenant tx routing.
    let store = quark::store::postgres::PostgresStore::open(&url, true)
        .await
        .unwrap();
    store.reset_for_tests().await.unwrap();
    let a = Arc::new(store) as Arc<dyn Store>;
    let t1 = a.clone().for_tenant(TenantId(1));
    let t2 = a.clone().for_tenant(TenantId(2));

    let r = rec("https://enforcement.example/p2a");
    t1.put_link(700, &r).await.unwrap();

    // Enforced by RLS (the tenant-tx sets app.tenant_id and the table is
    // FORCE'd), not merely by the WHERE predicate: tenant 1 sees its row,
    // tenant 2 sees nothing.
    assert!(
        t1.get_link(700).await.unwrap().is_some(),
        "owning tenant must see its own link"
    );
    assert!(
        t2.get_link(700).await.unwrap().is_none(),
        "other tenant must not see the link"
    );
    assert_eq!(
        t2.list_links(None, 100, None, None).await.unwrap().len(),
        0,
        "other tenant must list zero links"
    );
    assert_eq!(
        t1.list_links(None, 100, None, None).await.unwrap().len(),
        1,
        "owning tenant must list its own link"
    );
}

/// CRITICAL regression: `init_schema` must NOT `FORCE` RLS on
/// `click_counters`/`stats_meta`/`click_events` (analytics) or
/// `webhook_deliveries` (the cluster-wide outbox relay). Those accessors run
/// on the bare pool and never `SET LOCAL app.tenant_id`; if they were FORCE'd,
/// a non-superuser owner in cloud mode would see analytics writes/reads
/// silently return 0 rows, and `put_link_tx`/`put_alias_and_link_tx` would
/// have their delivery enqueue rejected by `WITH CHECK` (the delivery row
/// carries `tenant_id=0`, but the enclosing tenant-tx has
/// `app.tenant_id=<tenant>`, so `0 = <tenant>` fails for any non-default
/// tenant).
#[tokio::test]
async fn cloud_analytics_and_outbox_accessors_survive_force_rls() {
    let Some(url) = std::env::var("QUARK_TEST_DATABASE_URL").ok() else {
        eprintln!("skip: QUARK_TEST_DATABASE_URL not set");
        return;
    };
    let store = PostgresStore::open(&url, true).await.unwrap();
    store.reset_for_tests().await.unwrap();
    // `concrete` gives access to the `AnalyticsSink` inherent impl; `bare`
    // (the same store, as `Arc<dyn Store>`) gives access to the cluster-wide
    // outbox methods (`enqueue_deliveries`/`claim_due_deliveries`), which run
    // on the bare pool, tenant-less, exactly like production's relay.
    let concrete = Arc::new(store);
    let bare = concrete.clone() as Arc<dyn Store>;
    let t1 = bare.clone().for_tenant(TenantId(1));

    // A webhook subscription for tenant 1, subscribed to `link.created`.
    let sub_id = t1.next_webhook_id().await.unwrap();
    let sub = WebhookSubscription {
        id: sub_id,
        url: "https://enforcement.example/hook".to_string(),
        events: vec![EventType::LinkCreated],
        secret: "whsec_test".to_string(),
        active: true,
        created: 0,
        kind: SubscriptionKind::Generic,
    };
    t1.put_webhook(&sub).await.unwrap();

    // A link create that has a lifecycle delivery to enqueue: this is the
    // exact surface `put_link_tx` uses in production (redirect API's create
    // path). Before the fix, this INSERT into `webhook_deliveries` ran inside
    // the tenant-1 tx (`SET LOCAL app.tenant_id = 1`) but the delivery row's
    // `tenant_id` column defaults to 0 — under FORCE, `WITH CHECK` rejects it
    // (`0 = 1` is false).
    let link_id = 800u64;
    let delivery_key = format!("evt_enforcement.{sub_id}");
    let delivery = OutboxRow {
        delivery_key: delivery_key.clone(),
        subscription_id: sub_id,
        event_type: "link.created".to_string(),
        payload: "{}".to_string(),
        created: 0,
        next_attempt_at: 0,
    };
    t1.put_link_tx(
        link_id,
        &rec("https://enforcement.example/outbox"),
        &[delivery],
    )
    .await
    .expect("put_link_tx with a non-empty delivery must not be rejected by RLS");

    // The delivery actually landed and is claimable by the relay (bare pool,
    // no `app.tenant_id` set — must not be fail-closed by FORCE either).
    let claimed = bare.claim_due_deliveries(1, 10).await.unwrap();
    assert!(
        claimed.iter().any(|d| d.delivery_key == delivery_key),
        "delivery enqueued by put_link_tx must be claimable by the outbox relay"
    );

    // Analytics: `record_batch`/`stats` run on the bare pool too and must not
    // be fail-closed by FORCE.
    let click = ClickEvent {
        id: link_id,
        event_id: String::new(),
        ts: 1,
        referer: None,
        country: Some("BR".into()),
        user_agent: None,
        city: None,
        bot: false,
        ip: None,
        fbc: None,
        variant: None,
    };
    concrete
        .record_batch(&[click])
        .await
        .expect("record_batch must not be rejected by RLS");
    let stats = concrete
        .stats(link_id)
        .await
        .expect("stats must not be rejected by RLS");
    assert!(
        stats.is_some(),
        "stats must read back the just-recorded click (no spurious 0-rows under FORCE)"
    );
}
