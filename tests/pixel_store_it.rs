use quark::pixel::{PixelConfig, PixelCredentials, Provider};
use quark::store::postgres::PostgresStore;
use quark::store::Store;
use serial_test::serial;

async fn fresh() -> Option<PostgresStore> {
    let url = std::env::var("QUARK_TEST_DATABASE_URL").ok()?;
    let s = PostgresStore::open(&url, false).await.unwrap();
    s.reset_for_tests().await.unwrap();
    Some(s)
}

#[tokio::test]
#[serial(pg)]
async fn next_pixel_id_increments_pg() {
    let Some(s) = fresh().await else {
        eprintln!("skip: QUARK_TEST_DATABASE_URL not set");
        return;
    };
    let a = s
        .next_pixel_id(quark::tenant::DEFAULT_TENANT)
        .await
        .unwrap();
    let b = s
        .next_pixel_id(quark::tenant::DEFAULT_TENANT)
        .await
        .unwrap();
    assert_eq!(b, a + 1);
}

#[tokio::test]
#[serial(pg)]
async fn pixel_round_trip_pg() {
    let Some(s) = fresh().await else {
        return;
    };
    let config = PixelConfig {
        id: 1,
        provider: Provider::Ga4,
        credentials: PixelCredentials {
            measurement_id: Some("G-1".into()),
            api_secret: Some("s".into()),
            pixel_id: None,
            access_token: None,
        },
        active: true,
        created: 42,
    };
    s.put_pixel(quark::tenant::DEFAULT_TENANT, &config)
        .await
        .unwrap();

    let got = s
        .get_pixel(quark::tenant::DEFAULT_TENANT, 1)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(got.provider, Provider::Ga4);
    assert_eq!(got.credentials.measurement_id.as_deref(), Some("G-1"));
    assert!(got.active);
    assert_eq!(got.created, 42);

    assert!(s
        .get_pixel(quark::tenant::DEFAULT_TENANT, 999)
        .await
        .unwrap()
        .is_none());

    let list = s.list_pixels(quark::tenant::DEFAULT_TENANT).await.unwrap();
    assert_eq!(list.len(), 1);
    assert_eq!(list[0].id, 1);

    assert!(s
        .delete_pixel(quark::tenant::DEFAULT_TENANT, 1)
        .await
        .unwrap());
    assert!(!s
        .delete_pixel(quark::tenant::DEFAULT_TENANT, 1)
        .await
        .unwrap());
    assert!(s
        .get_pixel(quark::tenant::DEFAULT_TENANT, 1)
        .await
        .unwrap()
        .is_none());
    assert!(s
        .list_pixels(quark::tenant::DEFAULT_TENANT)
        .await
        .unwrap()
        .is_empty());
}

#[tokio::test]
#[serial(pg)]
async fn pixel_put_upserts_pg() {
    let Some(s) = fresh().await else {
        return;
    };
    let mut config = PixelConfig {
        id: 1,
        provider: Provider::MetaCapi,
        credentials: PixelCredentials {
            measurement_id: None,
            api_secret: None,
            pixel_id: Some("p1".into()),
            access_token: Some("t1".into()),
        },
        active: true,
        created: 1,
    };
    s.put_pixel(quark::tenant::DEFAULT_TENANT, &config)
        .await
        .unwrap();

    config.active = false;
    config.credentials.access_token = Some("t2".into());
    s.put_pixel(quark::tenant::DEFAULT_TENANT, &config)
        .await
        .unwrap();

    let got = s
        .get_pixel(quark::tenant::DEFAULT_TENANT, 1)
        .await
        .unwrap()
        .unwrap();
    assert!(!got.active);
    assert_eq!(got.credentials.access_token.as_deref(), Some("t2"));
    assert_eq!(
        s.list_pixels(quark::tenant::DEFAULT_TENANT)
            .await
            .unwrap()
            .len(),
        1
    );
}
