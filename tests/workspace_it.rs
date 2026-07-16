use quark::store::{postgres::PostgresStore, Store};
use serial_test::serial;

async fn fresh() -> Option<PostgresStore> {
    let url = std::env::var("QUARK_TEST_DATABASE_URL").ok()?;
    let s = PostgresStore::open(&url, true).await.unwrap();
    s.reset_for_tests().await.unwrap();
    Some(s)
}

// ids are >=1 and monotonic, never 0 (0 is the seeded default tenant).
#[tokio::test]
#[serial]
async fn next_tenant_id_starts_above_default() {
    let Some(store) = fresh().await else {
        return;
    };
    let a = store.next_tenant_id().await.unwrap();
    let b = store.next_tenant_id().await.unwrap();
    assert!(
        a >= 1 && b > a,
        "tenant ids must be >=1 (0 is the default tenant) and monotonic"
    );
}
