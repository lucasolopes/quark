use quark::analytics::{AnalyticsSink, ClickEvent};
use quark::store::postgres::PostgresStore;
use quark::store::{Record, Store};
use quark::tenant::TenantId;
use serial_test::serial;
use sqlx::postgres::PgPoolOptions;
use sqlx::{PgPool, Row};

async fn fresh() -> Option<PostgresStore> {
    let url = std::env::var("QUARK_TEST_DATABASE_URL").ok()?;
    let s = PostgresStore::open(&url, false).await.unwrap();
    s.reset_for_tests().await.unwrap();
    Some(s)
}

/// `fresh()` plus a raw pool used to inspect `tenant_id` columns directly —
/// `AnalyticsSink`/`Store` don't expose that column, only the app-level
/// aggregates that never surface it.
async fn fresh_with_pool() -> Option<(PostgresStore, PgPool)> {
    let url = std::env::var("QUARK_TEST_DATABASE_URL").ok()?;
    let s = PostgresStore::open(&url, false).await.unwrap();
    s.reset_for_tests().await.unwrap();
    let pool = PgPoolOptions::new()
        .max_connections(4)
        .connect(&url)
        .await
        .unwrap();
    Some((s, pool))
}

fn rec_for(tenant: TenantId, url: &str) -> Record {
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
        tenant_id: tenant,
    }
}

async fn click_events_tenant_id(pool: &PgPool, id: u64) -> i64 {
    sqlx::query("SELECT tenant_id FROM click_events WHERE id=$1 LIMIT 1")
        .bind(id as i64)
        .fetch_one(pool)
        .await
        .unwrap()
        .try_get("tenant_id")
        .unwrap()
}

async fn click_counters_tenant_id(pool: &PgPool, id: u64) -> i64 {
    sqlx::query("SELECT tenant_id FROM click_counters WHERE id=$1 LIMIT 1")
        .bind(id as i64)
        .fetch_one(pool)
        .await
        .unwrap()
        .try_get("tenant_id")
        .unwrap()
}

async fn stats_meta_tenant_id(pool: &PgPool, id: u64) -> i64 {
    sqlx::query("SELECT tenant_id FROM stats_meta WHERE id=$1")
        .bind(id as i64)
        .fetch_one(pool)
        .await
        .unwrap()
        .try_get("tenant_id")
        .unwrap()
}
fn ev(id: u64, ts: u64) -> ClickEvent {
    ClickEvent {
        id,
        event_id: String::new(),
        ts,
        referer: None,
        country: Some("BR".into()),
        user_agent: Some("iPhone".into()),
        city: None,
        bot: false,
        ip: None,
        fbc: None,
        variant: None,
        tenant_id: 0,
    }
}

fn ev_ua(id: u64, ts: u64, country: &str, ua: &str) -> ClickEvent {
    ClickEvent {
        id,
        event_id: String::new(),
        ts,
        referer: None,
        country: Some(country.into()),
        user_agent: Some(ua.into()),
        city: None,
        bot: false,
        ip: None,
        fbc: None,
        variant: None,
        tenant_id: 0,
    }
}

#[tokio::test]
#[serial(pg)]
async fn record_and_stats_pg() {
    let Some(s) = fresh().await else {
        eprintln!("skip: QUARK_TEST_DATABASE_URL not set");
        return;
    };
    s.record_batch(&[ev(1, 1_752_300_000), ev(1, 1_752_300_050)])
        .await
        .unwrap();
    let st = s.stats(1).await.unwrap().unwrap();
    assert_eq!(st.aggregates.total, 2);
    assert_eq!(st.aggregates.per_country.get("BR"), Some(&2));
    assert_eq!(st.recent.len(), 2);
    assert!(s.stats(999).await.unwrap().is_none());
}

#[tokio::test]
#[serial(pg)]
async fn retention_pg() {
    let Some(s) = fresh().await else {
        eprintln!("skip: QUARK_TEST_DATABASE_URL not set");
        return;
    };
    for b in 0..12u64 {
        let evs: Vec<ClickEvent> = (0..100)
            .map(|i| ev(7, 1_752_300_000 + b * 100 + i))
            .collect();
        s.record_batch(&evs).await.unwrap();
    }
    let st = s.stats(7).await.unwrap().unwrap();
    assert_eq!(st.aggregates.total, 1200);
    assert_eq!(st.recent.len(), 1000);
}

#[tokio::test]
#[serial(pg)]
async fn per_dimension_aggregation_across_batches_pg() {
    let Some(s) = fresh().await else {
        eprintln!("skip: QUARK_TEST_DATABASE_URL not set");
        return;
    };
    s.record_batch(&[
        ev_ua(3, 1_752_300_000, "BR", "Mozilla/5.0 (iPhone)"),
        ev_ua(3, 1_752_300_050, "US", "Mozilla/5.0 (Windows NT 10.0)"),
    ])
    .await
    .unwrap();
    s.record_batch(&[ev_ua(3, 1_752_300_100, "BR", "Mozilla/5.0 (iPhone)")])
        .await
        .unwrap();
    let st = s.stats(3).await.unwrap().unwrap();
    assert_eq!(st.aggregates.total, 3);
    assert_eq!(st.aggregates.per_country.get("BR"), Some(&2));
    assert_eq!(st.aggregates.per_country.get("US"), Some(&1));
    assert_eq!(st.aggregates.per_device.get("Mobile"), Some(&2));
    assert_eq!(st.aggregates.per_device.get("Desktop"), Some(&1));
    assert_eq!(st.aggregates.first_ts, 1_752_300_000);
    assert_eq!(st.aggregates.last_ts, 1_752_300_100);
    assert_eq!(st.aggregates.per_day.values().sum::<u64>(), 3);
}

#[tokio::test]
#[serial(pg)]
async fn bots_counted_in_total_excluded_from_breakdowns_pg() {
    let Some(s) = fresh().await else {
        eprintln!("skip: QUARK_TEST_DATABASE_URL not set");
        return;
    };
    s.record_batch(&[
        ev_ua(9, 1_752_300_000, "BR", "Mozilla/5.0 (iPhone)"),
        ev_ua(9, 1_752_300_050, "US", "Mozilla/5.0 (Windows NT 10.0)"),
        ev_ua(9, 1_752_300_100, "JP", "Googlebot/2.1"),
    ])
    .await
    .unwrap();
    let st = s.stats(9).await.unwrap().unwrap();
    assert_eq!(st.aggregates.total, 3);
    assert_eq!(st.aggregates.bots, 1);
    assert!(
        !st.aggregates.per_country.contains_key("JP"),
        "bot's country must not appear in per_country"
    );
    assert_eq!(st.aggregates.per_country.get("BR"), Some(&1));
    assert_eq!(st.aggregates.per_country.get("US"), Some(&1));
    assert_eq!(st.aggregates.per_device.values().sum::<u64>(), 2);
}

#[tokio::test]
#[serial(pg)]
async fn retention_keeps_newest_pg() {
    let Some(s) = fresh().await else {
        eprintln!("skip: QUARK_TEST_DATABASE_URL not set");
        return;
    };
    for b in 0..12u64 {
        let evs: Vec<ClickEvent> = (0..100)
            .map(|i| ev(11, 1_752_300_000 + b * 100 + i))
            .collect();
        s.record_batch(&evs).await.unwrap();
    }
    let st = s.stats(11).await.unwrap().unwrap();
    assert_eq!(st.recent.len(), 1000);
    let newest_ts = 1_752_300_000 + 11 * 100 + 99;
    let oldest_kept_ts = newest_ts - 999;
    assert_eq!(
        st.recent.last().unwrap().ts,
        newest_ts,
        "newest event must be retained"
    );
    assert_eq!(
        st.recent.first().unwrap().ts,
        oldest_kept_ts,
        "recent must hold the newest EVENTS_MAX, oldest dropped"
    );
}

#[tokio::test]
#[serial(pg)]
async fn stats_none_when_empty_pg() {
    let Some(s) = fresh().await else {
        eprintln!("skip: QUARK_TEST_DATABASE_URL not set");
        return;
    };
    assert!(s.stats(12345).await.unwrap().is_none());
}

#[tokio::test]
#[serial(pg)]
async fn record_batch_concurrent_no_lost_updates() {
    let url = match std::env::var("QUARK_TEST_DATABASE_URL") {
        Ok(u) => u,
        Err(_) => return,
    };
    let s0 = PostgresStore::open(&url, false).await.unwrap();
    s0.reset_for_tests().await.unwrap();
    let s1 = std::sync::Arc::new(PostgresStore::open(&url, false).await.unwrap());
    let s2 = std::sync::Arc::new(PostgresStore::open(&url, false).await.unwrap());
    let n = 50u64;
    let t1 = {
        let s = s1.clone();
        tokio::spawn(async move {
            for i in 0..n {
                s.record_batch(&[ev(42, 1_752_300_000 + i)]).await.unwrap();
            }
        })
    };
    let t2 = {
        let s = s2.clone();
        tokio::spawn(async move {
            for i in 0..n {
                s.record_batch(&[ev(42, 1_752_400_000 + i)]).await.unwrap();
            }
        })
    };
    t1.await.unwrap();
    t2.await.unwrap();
    let st = s0.stats(42).await.unwrap().unwrap();
    assert_eq!(st.aggregates.total, 2 * n);
}

/// Multi-tenancy P4a Task 1: `record_batch` binds `ev.tenant_id` into the 3
/// analytics tables, so a click for a link owned by tenant B lands tagged
/// with B — not the column default (0).
#[tokio::test]
#[serial(pg)]
async fn click_event_tagged_with_owning_tenant_pg() {
    let Some((s, pool)) = fresh_with_pool().await else {
        eprintln!("skip: QUARK_TEST_DATABASE_URL not set");
        return;
    };
    let tenant_b = TenantId(2);
    s.put_link(tenant_b, 50, &rec_for(tenant_b, "https://example.com/b"))
        .await
        .unwrap();

    let mut click = ev(50, 1_752_300_000);
    click.tenant_id = tenant_b.0;
    s.record_batch(&[click]).await.unwrap();

    assert_eq!(click_events_tenant_id(&pool, 50).await, tenant_b.0 as i64);
    assert_eq!(click_counters_tenant_id(&pool, 50).await, tenant_b.0 as i64);
    assert_eq!(stats_meta_tenant_id(&pool, 50).await, tenant_b.0 as i64);
}

/// Multi-tenancy P4a Task 1: the boot backfill (run as part of `init_schema`,
/// under the advisory lock) fixes rows written before `tenant_id` was
/// populated — i.e. still at the column default (0) — from the owning
/// link. It only touches `tenant_id = 0` rows with a matching link: a link
/// deleted before the backfill runs leaves its clicks at 0, and re-running
/// the backfill (opening the store again — `init_schema` runs on every
/// `open`) is a no-op once caught up.
#[tokio::test]
#[serial(pg)]
async fn analytics_tenant_backfill_pg() {
    let Some((s, pool)) = fresh_with_pool().await else {
        eprintln!("skip: QUARK_TEST_DATABASE_URL not set");
        return;
    };
    let url = std::env::var("QUARK_TEST_DATABASE_URL").unwrap();
    let tenant_b = TenantId(3);
    s.put_link(
        tenant_b,
        60,
        &rec_for(tenant_b, "https://example.com/backfill"),
    )
    .await
    .unwrap();

    // Rows pre-dating the stamp: written directly at tenant_id = 0, as
    // `record_batch` would have left them before this task.
    sqlx::query(
        "INSERT INTO click_events (id, ts, referer, country, user_agent, city, variant, event_id, tenant_id) \
         VALUES ($1, 1, NULL, NULL, NULL, NULL, NULL, '', 0)",
    )
    .bind(60i64)
    .execute(&pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO click_counters (id, dimension, bucket, count, tenant_id) VALUES ($1, 'total', '', 1, 0)",
    )
    .bind(60i64)
    .execute(&pool)
    .await
    .unwrap();
    sqlx::query("INSERT INTO stats_meta (id, first_ts, last_ts, tenant_id) VALUES ($1, 1, 1, 0)")
        .bind(60i64)
        .execute(&pool)
        .await
        .unwrap();

    // Orphan: a click_events row whose link id has no match in `links`
    // (the link was deleted). Must stay at 0 — the backfill only follows
    // `id`s that still resolve.
    sqlx::query(
        "INSERT INTO click_events (id, ts, referer, country, user_agent, city, variant, event_id, tenant_id) \
         VALUES ($1, 1, NULL, NULL, NULL, NULL, NULL, '', 0)",
    )
    .bind(61i64)
    .execute(&pool)
    .await
    .unwrap();

    // Run the backfill: `init_schema` runs on every `PostgresStore::open`.
    PostgresStore::open(&url, false).await.unwrap();

    assert_eq!(click_events_tenant_id(&pool, 60).await, tenant_b.0 as i64);
    assert_eq!(click_counters_tenant_id(&pool, 60).await, tenant_b.0 as i64);
    assert_eq!(stats_meta_tenant_id(&pool, 60).await, tenant_b.0 as i64);
    assert_eq!(
        click_events_tenant_id(&pool, 61).await,
        0,
        "a click whose link was deleted has no match and stays at the default tenant"
    );

    // Idempotent: running the backfill again must not change an
    // already-correct value.
    PostgresStore::open(&url, false).await.unwrap();
    assert_eq!(click_events_tenant_id(&pool, 60).await, tenant_b.0 as i64);
}

/// Multi-tenancy P4a Task 2: `stats_for_tenant` sums across every link owned
/// by a tenant. Tenant A and tenant B each own a link; both get clicks;
/// `stats_for_tenant` for either tenant counts ONLY that tenant's clicks —
/// the isolation invariant, since `click_counters`/`stats_meta` are
/// NOT_FORCED and the app-level `WHERE tenant_id = $1` is the only thing
/// standing between this and a cross-tenant leak.
#[tokio::test]
#[serial(pg)]
async fn stats_for_tenant_isolates_across_tenants_pg() {
    let Some(s) = fresh().await else {
        eprintln!("skip: QUARK_TEST_DATABASE_URL not set");
        return;
    };
    let tenant_a = TenantId(10);
    let tenant_b = TenantId(11);
    s.put_link(tenant_a, 100, &rec_for(tenant_a, "https://example.com/a1"))
        .await
        .unwrap();
    s.put_link(tenant_a, 101, &rec_for(tenant_a, "https://example.com/a2"))
        .await
        .unwrap();
    s.put_link(tenant_b, 200, &rec_for(tenant_b, "https://example.com/b1"))
        .await
        .unwrap();

    let mut a1 = ev_ua(100, 1_752_300_000, "BR", "Mozilla/5.0 (iPhone)");
    a1.tenant_id = tenant_a.0;
    let mut a2 = ev_ua(101, 1_752_300_050, "US", "Mozilla/5.0 (Windows NT 10.0)");
    a2.tenant_id = tenant_a.0;
    let mut b1 = ev_ua(200, 1_752_300_100, "JP", "Mozilla/5.0 (iPhone)");
    b1.tenant_id = tenant_b.0;
    let mut b2 = ev_ua(200, 1_752_300_150, "JP", "Mozilla/5.0 (iPhone)");
    b2.tenant_id = tenant_b.0;

    s.record_batch(&[a1, a2]).await.unwrap();
    s.record_batch(&[b1, b2]).await.unwrap();

    let agg_a = s.stats_for_tenant(tenant_a.0).await.unwrap();
    assert_eq!(
        agg_a.total, 2,
        "tenant A's aggregate counts only A's clicks"
    );
    assert_eq!(agg_a.per_country.get("BR"), Some(&1));
    assert_eq!(agg_a.per_country.get("US"), Some(&1));
    assert!(
        !agg_a.per_country.contains_key("JP"),
        "tenant A must never see tenant B's country breakdown"
    );

    let agg_b = s.stats_for_tenant(tenant_b.0).await.unwrap();
    assert_eq!(
        agg_b.total, 2,
        "tenant B's aggregate counts only B's clicks"
    );
    assert_eq!(agg_b.per_country.get("JP"), Some(&2));
    assert!(
        !agg_b.per_country.contains_key("BR") && !agg_b.per_country.contains_key("US"),
        "tenant B must never see tenant A's country breakdown"
    );
}

/// OSS/single-tenant parity: `stats_for_tenant(0)` (the default tenant)
/// aggregates every click recorded under it, same as the pre-P4a behavior
/// where there was only ever one tenant.
#[tokio::test]
#[serial(pg)]
async fn stats_for_tenant_default_tenant_aggregates_all_pg() {
    let Some(s) = fresh().await else {
        eprintln!("skip: QUARK_TEST_DATABASE_URL not set");
        return;
    };
    s.record_batch(&[ev(300, 1_752_300_000), ev(300, 1_752_300_050)])
        .await
        .unwrap();
    s.record_batch(&[ev(301, 1_752_300_100)]).await.unwrap();

    let agg = s.stats_for_tenant(0).await.unwrap();
    assert_eq!(agg.total, 3);
    assert_eq!(agg.per_country.get("BR"), Some(&3));
    assert_eq!(agg.first_ts, 1_752_300_000);
    assert_eq!(agg.last_ts, 1_752_300_100);
}

/// Regression: `stats(id)` (the per-link view) is untouched by Task 2 —
/// still keyed by `id` alone, unaffected by other tenants' or other links'
/// clicks.
#[tokio::test]
#[serial(pg)]
async fn stats_per_link_unchanged_by_tenant_aggregate_pg() {
    let Some(s) = fresh().await else {
        eprintln!("skip: QUARK_TEST_DATABASE_URL not set");
        return;
    };
    let tenant_a = TenantId(20);
    let tenant_b = TenantId(21);
    s.put_link(tenant_a, 400, &rec_for(tenant_a, "https://example.com/a"))
        .await
        .unwrap();
    s.put_link(tenant_b, 401, &rec_for(tenant_b, "https://example.com/b"))
        .await
        .unwrap();

    let mut ea = ev(400, 1_752_300_000);
    ea.tenant_id = tenant_a.0;
    let mut eb = ev(401, 1_752_300_050);
    eb.tenant_id = tenant_b.0;
    s.record_batch(&[ea, eb]).await.unwrap();

    let st_a = s.stats(400).await.unwrap().unwrap();
    assert_eq!(
        st_a.aggregates.total, 1,
        "per-link stats(id) stays keyed by id"
    );
    let st_b = s.stats(401).await.unwrap().unwrap();
    assert_eq!(st_b.aggregates.total, 1);
}
