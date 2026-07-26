//! The Google Sheets/Drive HTTP calls behind a trait, so the sync logic can be
//! driven by a mock in tests with no real credentials. The real implementation
//! targets fixed Google hosts, so there is no user-controlled URL and no SSRF
//! surface.

use serde_json::json;

/// Why a Google Sheets API call failed.
///
/// Typed rather than a `String` so the caller can tell a transport failure (the
/// request never landed, so a retry may work) from a rejection by Google (it
/// landed and was refused, so retrying the same call will not help). The sync
/// currently renders all of them into its status field, but the distinction is
/// what makes a retry policy possible without re-parsing text.
#[non_exhaustive]
#[derive(Debug, thiserror::Error)]
pub enum SheetsApiError {
    #[error("request to the sheets api failed")]
    Transport(#[from] reqwest::Error),
    #[error("{operation} returned status {status}")]
    Status {
        operation: &'static str,
        status: reqwest::StatusCode,
    },
    #[error("{operation}: response was missing `{field}`")]
    MalformedResponse {
        operation: &'static str,
        field: &'static str,
    },
}

/// The two Google API operations the sync needs. `access_token` is a short-lived
/// bearer token obtained from a refresh token.
#[async_trait::async_trait]
pub trait SheetsApi: Send + Sync + 'static {
    /// Creates a spreadsheet titled `title` and returns its id.
    async fn create_spreadsheet(
        &self,
        access_token: &str,
        title: &str,
    ) -> Result<String, SheetsApiError>;
    /// Overwrites the first sheet's values with `rows` (row-major, from A1),
    /// clearing any prior data first so a shorter catalog leaves no stale rows.
    async fn update_values(
        &self,
        access_token: &str,
        spreadsheet_id: &str,
        rows: &[Vec<String>],
    ) -> Result<(), SheetsApiError>;
}

/// The real implementation over reqwest.
pub struct GoogleSheetsApi {
    pub client: reqwest::Client,
}

/// Request budget for Google Sheets API calls. A sync reads whole ranges, so the
/// budget is generous; the lease TTL is what bounds a stuck sync overall.
const SHEETS_HTTP_TIMEOUT_SECS: u64 = 30;

/// Builds the HTTP client for Google API calls with a request timeout, so a
/// stalled connection cannot hang a sync (and hold the sync lease) forever.
#[expect(
    clippy::expect_used,
    reason = "a client with only timeouts and a redirect policy always builds"
)]
pub fn http_client() -> reqwest::Client {
    reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(SHEETS_HTTP_TIMEOUT_SECS))
        .connect_timeout(std::time::Duration::from_secs(
            crate::HTTP_CONNECT_TIMEOUT_SECS,
        ))
        .build()
        .expect("reqwest client builds")
}

const SHEETS_BASE: &str = "https://sheets.googleapis.com/v4/spreadsheets";

#[async_trait::async_trait]
impl SheetsApi for GoogleSheetsApi {
    async fn create_spreadsheet(
        &self,
        access_token: &str,
        title: &str,
    ) -> Result<String, SheetsApiError> {
        let resp = self
            .client
            .post(SHEETS_BASE)
            .bearer_auth(access_token)
            .json(&json!({ "properties": { "title": title } }))
            .send()
            .await?;
        if !resp.status().is_success() {
            return Err(SheetsApiError::Status {
                operation: "create_spreadsheet",
                status: resp.status(),
            });
        }
        let v: serde_json::Value = resp.json().await?;
        v.get("spreadsheetId")
            .and_then(|x| x.as_str())
            .map(String::from)
            .ok_or(SheetsApiError::MalformedResponse {
                operation: "create_spreadsheet",
                field: "spreadsheetId",
            })
    }

    async fn update_values(
        &self,
        access_token: &str,
        spreadsheet_id: &str,
        rows: &[Vec<String>],
    ) -> Result<(), SheetsApiError> {
        // Clear a generous range first so a re-sync of a smaller catalog does not
        // leave ghost rows behind, then write from A1.
        let clear_url = format!("{SHEETS_BASE}/{spreadsheet_id}/values/A1%3AZZ100000:clear");
        let cleared = self
            .client
            .post(&clear_url)
            .bearer_auth(access_token)
            .json(&json!({}))
            .send()
            .await?;
        if !cleared.status().is_success() {
            return Err(SheetsApiError::Status {
                operation: "clear",
                status: cleared.status(),
            });
        }
        let update_url = format!("{SHEETS_BASE}/{spreadsheet_id}/values/A1?valueInputOption=RAW");
        let updated = self
            .client
            .put(&update_url)
            .bearer_auth(access_token)
            .json(&json!({ "values": rows }))
            .send()
            .await?;
        if !updated.status().is_success() {
            return Err(SheetsApiError::Status {
                operation: "update_values",
                status: updated.status(),
            });
        }
        Ok(())
    }
}
