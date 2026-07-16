//! The Google Sheets/Drive HTTP calls behind a trait, so the sync logic can be
//! driven by a mock in tests with no real credentials. The real implementation
//! targets fixed Google hosts, so there is no user-controlled URL and no SSRF
//! surface.

use async_trait::async_trait;
use serde_json::json;

/// The two Google API operations the sync needs. `access_token` is a short-lived
/// bearer token obtained from a refresh token.
#[async_trait]
pub trait SheetsApi: Send + Sync {
    /// Creates a spreadsheet titled `title` and returns its id.
    async fn create_spreadsheet(&self, access_token: &str, title: &str) -> Result<String, String>;
    /// Overwrites the first sheet's values with `rows` (row-major, from A1),
    /// clearing any prior data first so a shorter catalog leaves no stale rows.
    async fn update_values(
        &self,
        access_token: &str,
        spreadsheet_id: &str,
        rows: &[Vec<String>],
    ) -> Result<(), String>;
}

/// The real implementation over reqwest.
pub struct GoogleSheetsApi {
    pub client: reqwest::Client,
}

/// Builds the HTTP client for Google API calls with a request timeout, so a
/// stalled connection cannot hang a sync (and hold the sync lease) forever.
pub fn http_client() -> reqwest::Client {
    reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(30))
        .build()
        .expect("reqwest client builds")
}

const SHEETS_BASE: &str = "https://sheets.googleapis.com/v4/spreadsheets";

#[async_trait]
impl SheetsApi for GoogleSheetsApi {
    async fn create_spreadsheet(&self, access_token: &str, title: &str) -> Result<String, String> {
        let resp = self
            .client
            .post(SHEETS_BASE)
            .bearer_auth(access_token)
            .json(&json!({ "properties": { "title": title } }))
            .send()
            .await
            .map_err(|e| e.to_string())?;
        if !resp.status().is_success() {
            return Err(format!("create_spreadsheet failed: {}", resp.status()));
        }
        let v: serde_json::Value = resp.json().await.map_err(|e| e.to_string())?;
        v.get("spreadsheetId")
            .and_then(|x| x.as_str())
            .map(String::from)
            .ok_or_else(|| "create_spreadsheet: response had no spreadsheetId".to_string())
    }

    async fn update_values(
        &self,
        access_token: &str,
        spreadsheet_id: &str,
        rows: &[Vec<String>],
    ) -> Result<(), String> {
        // Clear a generous range first so a re-sync of a smaller catalog does not
        // leave ghost rows behind, then write from A1.
        let clear_url = format!("{SHEETS_BASE}/{spreadsheet_id}/values/A1%3AZZ100000:clear");
        let cleared = self
            .client
            .post(&clear_url)
            .bearer_auth(access_token)
            .json(&json!({}))
            .send()
            .await
            .map_err(|e| e.to_string())?;
        if !cleared.status().is_success() {
            return Err(format!("clear failed: {}", cleared.status()));
        }
        let update_url = format!("{SHEETS_BASE}/{spreadsheet_id}/values/A1?valueInputOption=RAW");
        let updated = self
            .client
            .put(&update_url)
            .bearer_auth(access_token)
            .json(&json!({ "values": rows }))
            .send()
            .await
            .map_err(|e| e.to_string())?;
        if !updated.status().is_success() {
            return Err(format!("update_values failed: {}", updated.status()));
        }
        Ok(())
    }
}
