use async_trait::async_trait;
use quark::sheets::client::SheetsApi;
use quark::sheets::{sync, SheetsConnection, SyncStatus};
use quark::store::{open_backends, Record};
use quark::tenant::{Tenant, TenantId, DEFAULT_TENANT};
use std::sync::{Arc, Mutex};

struct MockApi {
    rows: Arc<Mutex<Vec<Vec<String>>>>,
}

#[async_trait]
impl SheetsApi for MockApi {
    async fn create_spreadsheet(&self, _tok: &str, _title: &str) -> Result<String, String> {
        Ok("sheet-1".to_string())
    }
    async fn update_values(
        &self,
        _tok: &str,
        _sid: &str,
        rows: &[Vec<String>],
    ) -> Result<(), String> {
        *self.rows.lock().unwrap() = rows.to_vec();
        Ok(())
    }
}

fn rec(url: &str, tenant: TenantId) -> Record {
    Record {
        url: url.into(),
        expiry: None,
        created: 1_700_000_000,
        tags: vec![],
        max_visits: None,
        rules: vec![],
        variants: vec![],
        app_ios: None,
        app_android: None,
        folder: None,
        fallback_url: None,
        password_hash: None,
        tenant_id: tenant,
    }
}

#[tokio::test]
async fn sync_reads_catalog_of_the_given_tenant_only() {
    let dir = tempfile::tempdir().unwrap();
    let (store, _sink) = open_backends(dir.path(), true).await.unwrap();

    let tenant_b = Tenant {
        id: TenantId(1),
        name: "Tenant B".into(),
        slug: "tenant-b".into(),
        created: 0,
    };
    store.put_tenant(&tenant_b).await.unwrap();

    // Tenant 0 (default) tem um link; tenant 1 tem outro.
    store
        .put_link(
            DEFAULT_TENANT,
            1,
            &rec("https://tenant0.example", DEFAULT_TENANT),
        )
        .await
        .unwrap();
    store
        .put_link(TenantId(1), 2, &rec("https://tenant1.example", TenantId(1)))
        .await
        .unwrap();

    let rows = Arc::new(Mutex::new(Vec::new()));
    let api = MockApi { rows: rows.clone() };
    let mut conn = SheetsConnection {
        refresh_token: "rt".into(),
        email: "b@example.com".into(),
        spreadsheet_id: None,
        last_sync: None,
        last_status: SyncStatus::Never,
    };

    sync(
        &store,
        &api,
        0x1234,
        "https://s.example",
        &mut conn,
        "access-token",
        1_752_300_000,
        TenantId(1),
    )
    .await
    .unwrap();

    let written = rows.lock().unwrap();
    let flat: String = written
        .iter()
        .flatten()
        .cloned()
        .collect::<Vec<_>>()
        .join("|");
    assert!(
        flat.contains("https://tenant1.example"),
        "deve conter o link do tenant 1: {flat}"
    );
    assert!(
        !flat.contains("https://tenant0.example"),
        "NÃO deve conter o link do tenant 0 (vazamento): {flat}"
    );
    assert_eq!(conn.last_status, SyncStatus::Ok);
}
