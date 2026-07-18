//! Native Google Sheets connector: mirrors the link catalog into a spreadsheet
//! the operator owns. OAuth Authorization Code (offline) obtains a refresh
//! token; the sync reads the catalog and overwrites the sheet. Opt-in via
//! `QUARK_SHEETS_CLIENT_ID`/`_CLIENT_SECRET`/`_REDIRECT_URL`; off otherwise.
//!
//! The Google HTTP calls live behind the [`client::SheetsApi`] trait so the sync
//! logic is testable with a mock and no real credentials. Google endpoints are
//! fixed hosts, so there is no SSRF surface (unlike webhook destinations).

pub mod client;

use serde::{Deserialize, Serialize};

/// The Drive scope requested: create and edit only the spreadsheets the app
/// itself creates. The most privacy-preserving Sheets/Drive scope.
pub const SHEETS_SCOPE: &str = "https://www.googleapis.com/auth/drive.file";

/// The full scope string sent to Google: the Drive scope plus `openid email` so
/// the id_token carries the connected account's email (shown in the panel as
/// "connected as ..."). `openid`/`email` are basic, non-sensitive scopes.
pub const CONNECT_SCOPE: &str = "openid email https://www.googleapis.com/auth/drive.file";

const AUTH_ENDPOINT: &str = "https://accounts.google.com/o/oauth2/v2/auth";
const TOKEN_ENDPOINT: &str = "https://oauth2.googleapis.com/token";

/// Opt-in configuration for the Sheets connector. Present only when all three
/// OAuth values are set; `sync_secs` enables the scheduled refresh.
#[derive(Clone, Debug)]
pub struct SheetsConfig {
    pub client_id: String,
    pub client_secret: String,
    pub redirect_url: String,
    pub sync_secs: Option<u64>,
}

impl SheetsConfig {
    /// Reads the connector config from the environment. `None` (off) unless the
    /// client id, secret, and redirect URL are all present and non-empty.
    pub fn from_env() -> Option<SheetsConfig> {
        let sync = std::env::var("QUARK_SHEETS_SYNC_SECS")
            .ok()
            .and_then(|s| s.parse::<u64>().ok());
        Self::from_parts(
            &std::env::var("QUARK_SHEETS_CLIENT_ID").unwrap_or_default(),
            &std::env::var("QUARK_SHEETS_CLIENT_SECRET").unwrap_or_default(),
            &std::env::var("QUARK_SHEETS_REDIRECT_URL").unwrap_or_default(),
            sync,
        )
    }

    /// Builds a config from explicit parts (used by `from_env` and tests). The
    /// scheduled interval is floored to 60s to match the other timers.
    pub fn from_parts(
        id: &str,
        secret: &str,
        redirect: &str,
        sync_secs: Option<u64>,
    ) -> Option<SheetsConfig> {
        if id.is_empty() || secret.is_empty() || redirect.is_empty() {
            return None;
        }
        Some(SheetsConfig {
            client_id: id.to_string(),
            client_secret: secret.to_string(),
            redirect_url: redirect.to_string(),
            sync_secs: sync_secs.map(|s| s.max(60)),
        })
    }
}

/// The outcome of the most recent sync, surfaced in the panel.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "state", content = "detail", rename_all = "lowercase")]
pub enum SyncStatus {
    /// Connected but never synced yet.
    Never,
    /// The last sync succeeded.
    Ok,
    /// The last sync failed; the string is a short reason (never a token).
    Error(String),
}

/// A persisted Sheets connection. Stored per tenant: in OSS (single-tenant)
/// there is one operator with one Google account and one spreadsheet; in cloud
/// each tenant has its own connection, synced in isolation from the others.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SheetsConnection {
    pub refresh_token: String,
    pub email: String,
    pub spreadsheet_id: Option<String>,
    pub last_sync: Option<u64>,
    pub last_status: SyncStatus,
}

/// The Google authorization URL for a fresh connect attempt. Requests
/// `access_type=offline` + `prompt=consent` so Google returns a refresh token,
/// and the `drive.file` scope.
pub fn connect_url(cfg: &SheetsConfig, state: &str) -> String {
    let q = url::form_urlencoded::Serializer::new(String::new())
        .append_pair("response_type", "code")
        .append_pair("client_id", &cfg.client_id)
        .append_pair("redirect_uri", &cfg.redirect_url)
        .append_pair("scope", CONNECT_SCOPE)
        .append_pair("access_type", "offline")
        .append_pair("prompt", "consent")
        .append_pair("include_granted_scopes", "true")
        .append_pair("state", state)
        .finish();
    format!("{AUTH_ENDPOINT}?{q}")
}

/// The subset of Google's token response we use.
#[derive(Debug, Deserialize)]
pub struct TokenResponse {
    pub access_token: String,
    #[serde(default)]
    pub refresh_token: Option<String>,
    #[serde(default)]
    pub id_token: Option<String>,
}

/// Exchanges an authorization `code` for tokens (Authorization Code grant).
pub async fn exchange_code(
    client: &reqwest::Client,
    cfg: &SheetsConfig,
    code: &str,
) -> Result<TokenResponse, String> {
    let resp = client
        .post(TOKEN_ENDPOINT)
        .form(&[
            ("code", code),
            ("client_id", cfg.client_id.as_str()),
            ("client_secret", cfg.client_secret.as_str()),
            ("redirect_uri", cfg.redirect_url.as_str()),
            ("grant_type", "authorization_code"),
        ])
        .send()
        .await
        .map_err(|e| e.to_string())?;
    if !resp.status().is_success() {
        return Err(format!("token exchange failed: {}", resp.status()));
    }
    resp.json::<TokenResponse>()
        .await
        .map_err(|e| e.to_string())
}

/// Trades a stored refresh token for a fresh short-lived access token.
pub async fn refresh_access_token(
    client: &reqwest::Client,
    cfg: &SheetsConfig,
    refresh_token: &str,
) -> Result<String, String> {
    let resp = client
        .post(TOKEN_ENDPOINT)
        .form(&[
            ("refresh_token", refresh_token),
            ("client_id", cfg.client_id.as_str()),
            ("client_secret", cfg.client_secret.as_str()),
            ("grant_type", "refresh_token"),
        ])
        .send()
        .await
        .map_err(|e| e.to_string())?;
    if !resp.status().is_success() {
        return Err(format!("token refresh failed: {}", resp.status()));
    }
    let tok = resp
        .json::<TokenResponse>()
        .await
        .map_err(|e| e.to_string())?;
    Ok(tok.access_token)
}

/// The spreadsheet title quark creates on first sync.
const SPREADSHEET_TITLE: &str = "quark links";
/// Page size when reading the catalog for a sync.
const SYNC_PAGE: usize = 500;
/// Upper bound on links written in a single sync. A snapshot is one `values`
/// write, so a very large catalog risks an oversized request and heap. Beyond
/// this the sync fails with a clear status rather than OOMing or getting a
/// silent partial sheet; a larger catalog wants a chunked/streaming sync.
const MAX_SYNC_LINKS: usize = 100_000;

/// Builds the spreadsheet rows from the link catalog: a header row plus one row
/// per link. `visits` is `id -> visit count` (the same source the panel uses;
/// only visit-capped links accrue a count, so it matches the panel exactly).
pub fn catalog_rows(
    links: &[(u64, crate::store::Record)],
    key: u64,
    base_url: &str,
    visits: &std::collections::HashMap<u64, u64>,
) -> Vec<Vec<String>> {
    let mut rows: Vec<Vec<String>> = Vec::with_capacity(links.len() + 1);
    rows.push(
        [
            "code",
            "short_url",
            "destination",
            "created",
            "visits",
            "tags",
            "folder",
        ]
        .iter()
        .map(|s| s.to_string())
        .collect(),
    );
    for (id, rec) in links {
        let code = crate::codec::to_base62(crate::permute::encode(*id, key));
        rows.push(vec![
            code.clone(),
            format!("{base_url}/{code}"),
            rec.url.clone(),
            rec.created.to_string(),
            visits.get(id).copied().unwrap_or(0).to_string(),
            rec.tags.join(", "),
            rec.folder.clone().unwrap_or_default(),
        ]);
    }
    rows
}

/// Runs one sync: create the spreadsheet if needed, read the whole catalog, and
/// overwrite the sheet. `access_token` is already refreshed by the caller (the
/// refresh is a network call kept out of this function so it stays mockable).
/// Updates `conn` in place; the caller persists it.
// The parameters are all independent inputs (store, api, key, base URL, the
// connection, the access token, the clock, and the tenant); bundling them into
// a struct would not clarify the call sites, so allow the arity here.
#[allow(clippy::too_many_arguments)]
pub async fn sync(
    store: &std::sync::Arc<dyn crate::store::Store>,
    api: &dyn client::SheetsApi,
    key: u64,
    base_url: &str,
    conn: &mut SheetsConnection,
    access_token: &str,
    now: u64,
    tenant: crate::tenant::TenantId,
) -> Result<(), String> {
    if conn.spreadsheet_id.is_none() {
        let id = api
            .create_spreadsheet(access_token, SPREADSHEET_TITLE)
            .await?;
        conn.spreadsheet_id = Some(id);
    }
    let sid = conn
        .spreadsheet_id
        .clone()
        .expect("spreadsheet id set above");

    // Read the whole catalog by keyset pagination.
    let mut links: Vec<(u64, crate::store::Record)> = Vec::new();
    let mut after: Option<u64> = None;
    loop {
        let page = store
            .list_links(tenant, after, SYNC_PAGE, None, None)
            .await
            .map_err(|e| format!("list_links: {e:?}"))?;
        let got = page.len();
        if let Some((last_id, _)) = page.last() {
            after = Some(*last_id);
        }
        links.extend(page);
        if links.len() > MAX_SYNC_LINKS {
            return Err(format!(
                "catalog exceeds {MAX_SYNC_LINKS} links; snapshot sync not supported at this size"
            ));
        }
        if got < SYNC_PAGE {
            break;
        }
    }

    let mut visits = std::collections::HashMap::with_capacity(links.len());
    for (id, _) in &links {
        if let Ok(v) = store.visits(tenant, *id).await {
            visits.insert(*id, v);
        }
    }

    let rows = catalog_rows(&links, key, base_url, &visits);
    api.update_values(access_token, &sid, &rows).await?;
    conn.last_sync = Some(now);
    conn.last_status = SyncStatus::Ok;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn from_parts_is_none_without_all_required() {
        assert!(SheetsConfig::from_parts("", "s", "r", None).is_none());
        assert!(SheetsConfig::from_parts("i", "", "r", None).is_none());
        assert!(SheetsConfig::from_parts("i", "s", "", None).is_none());
        assert!(SheetsConfig::from_parts("i", "s", "r", None).is_some());
    }

    #[test]
    fn sync_secs_floored_to_60() {
        assert_eq!(
            SheetsConfig::from_parts("i", "s", "r", Some(5))
                .unwrap()
                .sync_secs,
            Some(60)
        );
        assert_eq!(
            SheetsConfig::from_parts("i", "s", "r", Some(3600))
                .unwrap()
                .sync_secs,
            Some(3600)
        );
        assert_eq!(
            SheetsConfig::from_parts("i", "s", "r", None)
                .unwrap()
                .sync_secs,
            None
        );
    }

    #[test]
    fn connect_url_requests_offline_consent_and_drive_file_scope() {
        let cfg = SheetsConfig::from_parts(
            "cid",
            "sec",
            "https://h/admin/integrations/sheets/callback",
            None,
        )
        .unwrap();
        let url = connect_url(&cfg, "st4te");
        assert!(url.starts_with("https://accounts.google.com/o/oauth2/v2/auth?"));
        assert!(url.contains("client_id=cid"));
        assert!(url.contains("access_type=offline"));
        assert!(url.contains("prompt=consent"));
        // The Drive scope is present; openid+email precede it (URL-encoded spaces).
        assert!(url.contains("drive.file"));
        assert!(url.contains("scope=openid+email+"));
        assert!(url.contains("state=st4te"));
        assert!(
            url.contains("redirect_uri=https%3A%2F%2Fh%2Fadmin%2Fintegrations%2Fsheets%2Fcallback")
        );
    }

    #[test]
    fn catalog_rows_has_header_then_one_row_per_link() {
        use crate::store::Record;
        let rec = |url: &str, tags: Vec<&str>, folder: Option<&str>| Record {
            url: url.into(),
            expiry: None,
            created: 1_700_000_000,
            tags: tags.into_iter().map(String::from).collect(),
            max_visits: None,
            rules: vec![],
            variants: vec![],
            app_ios: None,
            app_android: None,
            folder: folder.map(String::from),
            fallback_url: None,
            password_hash: None,
            tenant_id: crate::tenant::DEFAULT_TENANT,
        };
        let links = vec![
            (1u64, rec("https://a.com", vec!["x", "y"], Some("mkt"))),
            (2u64, rec("https://b.com", vec![], None)),
        ];
        let mut visits = std::collections::HashMap::new();
        visits.insert(1u64, 42u64);
        let rows = catalog_rows(&links, 0x1234, "https://s.example", &visits);
        assert_eq!(rows.len(), 3);
        assert_eq!(
            rows[0],
            vec![
                "code",
                "short_url",
                "destination",
                "created",
                "visits",
                "tags",
                "folder"
            ]
        );
        assert_eq!(rows[1][2], "https://a.com");
        assert_eq!(rows[1][3], "1700000000");
        assert_eq!(rows[1][4], "42");
        assert_eq!(rows[1][5], "x, y");
        assert_eq!(rows[1][6], "mkt");
        assert_eq!(rows[2][4], "0");
        assert!(rows[1][1].starts_with("https://s.example/"));
    }

    #[test]
    fn sync_status_serializes_tagged() {
        assert_eq!(
            serde_json::to_string(&SyncStatus::Ok).unwrap(),
            r#"{"state":"ok"}"#
        );
        assert_eq!(
            serde_json::to_string(&SyncStatus::Never).unwrap(),
            r#"{"state":"never"}"#
        );
        assert_eq!(
            serde_json::to_string(&SyncStatus::Error("boom".into())).unwrap(),
            r#"{"state":"error","detail":"boom"}"#
        );
    }
}
