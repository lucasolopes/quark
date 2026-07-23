//! Slack "Add to Slack" connector: an OAuth v2 install that returns an incoming
//! webhook URL, which quark persists as a `kind: Slack` webhook subscription.
//! There is no bespoke storage — the existing webhook delivery already formats
//! and posts channel payloads to Slack, so the OAuth flow only has to create the
//! subscription. Opt-in via `QUARK_SLACK_CLIENT_ID`/`_CLIENT_SECRET`/
//! `_REDIRECT_URL`; off otherwise.
//!
//! Slack's OAuth hosts are fixed, so there is no SSRF surface here (unlike an
//! arbitrary webhook destination). The returned webhook URL is on
//! `hooks.slack.com` and is still run through the normal webhook URL guard by
//! the caller before it is stored.

use serde::Deserialize;

/// The only scope quark requests: post messages through an incoming webhook the
/// installing user picks a channel for. Nothing else.
pub const SCOPE: &str = "incoming-webhook";

const AUTH_ENDPOINT: &str = "https://slack.com/oauth/v2/authorize";
const TOKEN_ENDPOINT: &str = "https://slack.com/api/oauth.v2.access";

/// Opt-in configuration for the Slack connector. Present only when all three
/// OAuth values are set.
#[derive(Clone, Debug)]
pub struct SlackConfig {
    pub client_id: String,
    pub client_secret: String,
    pub redirect_url: String,
}

impl SlackConfig {
    /// Reads the connector config from the environment. `None` (off) unless the
    /// client id, secret, and redirect URL are all present and non-empty.
    pub fn from_env() -> Option<SlackConfig> {
        Self::from_parts(
            &std::env::var("QUARK_SLACK_CLIENT_ID").unwrap_or_default(),
            &std::env::var("QUARK_SLACK_CLIENT_SECRET").unwrap_or_default(),
            &std::env::var("QUARK_SLACK_REDIRECT_URL").unwrap_or_default(),
        )
    }

    /// Builds a config from explicit parts (used by `from_env` and tests).
    pub fn from_parts(id: &str, secret: &str, redirect: &str) -> Option<SlackConfig> {
        if id.is_empty() || secret.is_empty() || redirect.is_empty() {
            return None;
        }
        Some(SlackConfig {
            client_id: id.to_string(),
            client_secret: secret.to_string(),
            redirect_url: redirect.to_string(),
        })
    }
}

/// The Slack authorization URL for an "Add to Slack" install. The user picks a
/// channel; Slack redirects back to `redirect_url` with `code` and `state`.
pub fn connect_url(cfg: &SlackConfig, state: &str) -> String {
    let q = url::form_urlencoded::Serializer::new(String::new())
        .append_pair("client_id", &cfg.client_id)
        .append_pair("scope", SCOPE)
        .append_pair("redirect_uri", &cfg.redirect_url)
        .append_pair("state", state)
        .finish();
    format!("{AUTH_ENDPOINT}?{q}")
}

/// The incoming-webhook object Slack returns on a successful install.
#[derive(Debug, Deserialize)]
pub struct IncomingWebhook {
    pub url: String,
    #[serde(default)]
    pub channel: Option<String>,
    #[serde(default)]
    pub configuration_url: Option<String>,
}

/// The subset of `oauth.v2.access` we use. Slack always returns HTTP 200; the
/// `ok` flag reports success, and `error` carries the reason on failure.
#[derive(Debug, Deserialize)]
pub struct OAuthAccess {
    pub ok: bool,
    #[serde(default)]
    pub error: Option<String>,
    #[serde(default)]
    pub incoming_webhook: Option<IncomingWebhook>,
}

/// Exchanges an authorization `code` for the install result (which carries the
/// incoming webhook URL). Sends the client credentials as form parameters, as
/// Slack's `oauth.v2.access` accepts.
pub async fn exchange_code(
    client: &reqwest::Client,
    cfg: &SlackConfig,
    code: &str,
) -> Result<OAuthAccess, String> {
    let resp = client
        .post(TOKEN_ENDPOINT)
        .form(&[
            ("code", code),
            ("client_id", cfg.client_id.as_str()),
            ("client_secret", cfg.client_secret.as_str()),
            ("redirect_uri", cfg.redirect_url.as_str()),
        ])
        .send()
        .await
        .map_err(|e| e.to_string())?;
    if !resp.status().is_success() {
        return Err(format!("slack oauth exchange failed: {}", resp.status()));
    }
    resp.json::<OAuthAccess>().await.map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cfg() -> SlackConfig {
        SlackConfig::from_parts("cid", "sec", "https://h/admin/integrations/slack/callback")
            .unwrap()
    }

    #[test]
    fn from_parts_requires_all_three() {
        assert!(SlackConfig::from_parts("", "s", "r").is_none());
        assert!(SlackConfig::from_parts("i", "", "r").is_none());
        assert!(SlackConfig::from_parts("i", "s", "").is_none());
        assert!(SlackConfig::from_parts("i", "s", "r").is_some());
    }

    #[test]
    fn connect_url_has_endpoint_scope_and_state() {
        let url = connect_url(&cfg(), "xyz");
        assert!(url.starts_with("https://slack.com/oauth/v2/authorize?"));
        assert!(url.contains("scope=incoming-webhook"));
        assert!(url.contains("client_id=cid"));
        assert!(url.contains("state=xyz"));
        assert!(
            url.contains("redirect_uri=https%3A%2F%2Fh%2Fadmin%2Fintegrations%2Fslack%2Fcallback")
        );
    }

    #[test]
    fn parses_successful_oauth_access() {
        let json = r##"{"ok":true,"incoming_webhook":{"url":"https://hooks.slack.com/services/T/B/x","channel":"#general","configuration_url":"https://team.slack.com/services/B"}}"##;
        let parsed: OAuthAccess = serde_json::from_str(json).unwrap();
        assert!(parsed.ok);
        assert_eq!(
            parsed.incoming_webhook.unwrap().url,
            "https://hooks.slack.com/services/T/B/x"
        );
    }

    #[test]
    fn parses_error_oauth_access() {
        let parsed: OAuthAccess =
            serde_json::from_str(r#"{"ok":false,"error":"invalid_code"}"#).unwrap();
        assert!(!parsed.ok);
        assert_eq!(parsed.error.as_deref(), Some("invalid_code"));
        assert!(parsed.incoming_webhook.is_none());
    }
}
