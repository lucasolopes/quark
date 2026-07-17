//! Server-side conversion forwarding (roadmap #14): pixel config type,
//! provider payload formatters (GA4 Measurement Protocol, Meta Conversions
//! API) and an injectable-base forwarder. Wiring into the analytics worker
//! and the admin endpoints is a follow-up task; this module is pure/mockable
//! on its own.

use crate::analytics::ClickEvent;
use crate::{codec, permute};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::sync::OnceLock;
use std::time::Duration;
use url::Url;

/// GA4 Measurement Protocol event name used for every forwarded click.
const GA4_EVENT_NAME: &str = "quark_click";
/// Meta Conversions API event name used for every forwarded click.
const META_EVENT_NAME: &str = "Lead";
/// Per-request timeout applied to provider calls (never blocks the redirect
/// hot path; this only runs from the async analytics worker).
const FORWARD_TIMEOUT: Duration = Duration::from_secs(5);

/// A conversion-forwarding provider.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Provider {
    Ga4,
    MetaCapi,
}

/// Provider credentials. Only the fields relevant to `provider` are used;
/// the others stay `None`. `serde(default)` keeps this forward-compatible
/// if a future field is added.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct PixelCredentials {
    #[serde(default)]
    pub measurement_id: Option<String>,
    #[serde(default)]
    pub api_secret: Option<String>,
    #[serde(default)]
    pub pixel_id: Option<String>,
    #[serde(default)]
    pub access_token: Option<String>,
}

/// Instance-level pixel/conversion-forwarding configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PixelConfig {
    pub id: u64,
    pub provider: Provider,
    pub credentials: PixelCredentials,
    pub active: bool,
    pub created: u64,
}

/// Error forwarding a batch to a provider. The caller (the analytics
/// worker, in a later task) fails open on this: a provider error never
/// affects redirects.
#[derive(Debug)]
pub enum PixelError {
    Http(reqwest::Error),
    Status(reqwest::StatusCode),
    InvalidBase,
}

impl std::fmt::Display for PixelError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            PixelError::Http(e) => write!(f, "http: {e}"),
            PixelError::Status(s) => write!(f, "provider returned status {s}"),
            PixelError::InvalidBase => write!(f, "invalid pixel provider base host"),
        }
    }
}
impl std::error::Error for PixelError {}

/// Fixed production provider hosts, injectable only so tests can point at a
/// local mock server. Production code always constructs this via `Default`
/// (no SSRF surface: the operator supplies credentials, not URLs).
#[derive(Debug, Clone)]
pub struct PixelBases {
    pub ga4: String,
    pub meta: String,
}

impl Default for PixelBases {
    fn default() -> Self {
        PixelBases {
            ga4: "https://www.google-analytics.com".to_string(),
            meta: "https://graph.facebook.com".to_string(),
        }
    }
}

impl PixelBases {
    /// Returns the configured base host for the given provider.
    pub fn base_for(&self, provider: Provider) -> &str {
        match provider {
            Provider::Ga4 => &self.ga4,
            Provider::MetaCapi => &self.meta,
        }
    }
}

/// A synthetic `client_id`, stable for the lifetime of this process
/// (batch/instance), carrying no real user identifier. Generated once from
/// the process start time.
fn instance_client_id() -> &'static str {
    static ID: OnceLock<String> = OnceLock::new();
    ID.get_or_init(|| {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        format!("quark-{:x}", nanos as u64)
    })
}

/// The real short code for a click, as seen by the end user
/// (`codec::to_base62(permute::encode(id, key))`), used as `link_code` in
/// both provider payloads instead of the raw internal id.
fn link_code(id: u64, key: u64) -> String {
    codec::to_base62(permute::encode(id, key))
}

/// Builds the GA4 Measurement Protocol batch body for a slice of events.
/// `client_id` is synthetic (per-instance), never a real user id. `link_code`
/// is the real short code (`key`-derived), not the internal numeric id.
pub fn ga4_payload(events: &[ClickEvent], key: u64) -> Value {
    let events_json: Vec<Value> = events
        .iter()
        .map(|e| {
            json!({
                "name": GA4_EVENT_NAME,
                "params": {
                    "link_code": link_code(e.id, key),
                    "country": e.country,
                    "transaction_id": e.event_id,
                },
            })
        })
        .collect();
    json!({
        "client_id": instance_client_id(),
        "events": events_json,
    })
}

/// Lowercase hex SHA-256 of the lowercased input. Used for the Meta
/// `user_data` fields the Conversions API requires hashed (e.g. country).
fn sha256_hex(s: &str) -> String {
    use sha2::{Digest, Sha256};
    let digest = Sha256::digest(s.to_lowercase().as_bytes());
    digest.iter().map(|b| format!("{b:02x}")).collect()
}

/// Builds the Meta Conversions API batch body for a slice of events.
/// `link_code` is the real short code (`key`-derived), not the internal
/// numeric id. Each event carries a `user_data` object with only the keys
/// present on the click: `client_ip_address`, `client_user_agent` and `fbc`
/// are sent plain (Meta hashes the IP server-side and the others are not
/// PII to hash), while `country` is sent as a SHA-256 hex of its lowercased
/// value (Meta requires that field hashed). Absent keys are omitted, never
/// emitted as null.
pub fn meta_payload(events: &[ClickEvent], key: u64) -> Value {
    let data: Vec<Value> = events
        .iter()
        .map(|e| {
            let mut user_data = serde_json::Map::new();
            if let Some(ip) = &e.ip {
                user_data.insert("client_ip_address".to_string(), json!(ip));
            }
            if let Some(ua) = &e.user_agent {
                user_data.insert("client_user_agent".to_string(), json!(ua));
            }
            if let Some(fbc) = &e.fbc {
                user_data.insert("fbc".to_string(), json!(fbc));
            }
            if let Some(country) = &e.country {
                user_data.insert("country".to_string(), json!(sha256_hex(country)));
            }
            json!({
                "event_name": META_EVENT_NAME,
                "event_time": e.ts,
                "event_id": e.event_id,
                "action_source": "website",
                "custom_data": {
                    "link_code": link_code(e.id, key),
                },
                "user_data": Value::Object(user_data),
            })
        })
        .collect();
    json!({ "data": data })
}

/// Builds the provider URL. `base` is injectable so tests can point at a
/// local mock server; production always passes the fixed provider host
/// (no SSRF surface: the operator supplies credentials, not URLs).
/// Credentials are percent-encoded via `url::Url` (query pairs / path
/// segments) so a credential containing `&`, `#` or a space can never break
/// the URL.
///
/// Returns `Err(PixelError::InvalidBase)` instead of panicking if `base` is
/// not a valid base URL. In production `base` is always one of the fixed
/// hosts in `PixelBases::default()`, so this never fires today, but a
/// future configurable base must not be able to panic the analytics worker.
pub fn provider_url(base: &str, config: &PixelConfig) -> Result<String, PixelError> {
    match config.provider {
        Provider::Ga4 => {
            let mut url = Url::parse(base).map_err(|_| PixelError::InvalidBase)?;
            url.set_path("/mp/collect");
            url.query_pairs_mut()
                .append_pair(
                    "measurement_id",
                    config.credentials.measurement_id.as_deref().unwrap_or(""),
                )
                .append_pair(
                    "api_secret",
                    config.credentials.api_secret.as_deref().unwrap_or(""),
                );
            Ok(url.to_string())
        }
        Provider::MetaCapi => {
            let mut url = Url::parse(base).map_err(|_| PixelError::InvalidBase)?;
            url.path_segments_mut()
                .map_err(|_| PixelError::InvalidBase)?
                .push("v19.0")
                .push(config.credentials.pixel_id.as_deref().unwrap_or(""))
                .push("events");
            url.query_pairs_mut().append_pair(
                "access_token",
                config.credentials.access_token.as_deref().unwrap_or(""),
            );
            Ok(url.to_string())
        }
    }
}

/// Forwards a batch of click events to a single pixel config. Async only:
/// callers must run this off the redirect hot path (the analytics worker).
/// Fails open at the caller: an `Err` here must never affect a redirect.
/// `key` derives the real short code used as `link_code` in the payload.
pub async fn forward(
    client: &reqwest::Client,
    base: &str,
    config: &PixelConfig,
    events: &[ClickEvent],
    key: u64,
) -> Result<(), PixelError> {
    if events.is_empty() {
        return Ok(());
    }
    let payload = match config.provider {
        Provider::Ga4 => ga4_payload(events, key),
        Provider::MetaCapi => meta_payload(events, key),
    };
    let url = provider_url(base, config)?;
    let resp = client
        .post(url)
        .json(&payload)
        .timeout(FORWARD_TIMEOUT)
        .send()
        .await
        .map_err(|e| PixelError::Http(e.without_url()))?;
    if !resp.status().is_success() {
        return Err(PixelError::Status(resp.status()));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::extract::State;
    use axum::routing::post;
    use axum::Router;
    use std::sync::{Arc, Mutex};
    use tokio::net::TcpListener;

    fn ev(id: u64, ts: u64, country: Option<&str>) -> ClickEvent {
        ClickEvent {
            id,
            event_id: format!("clk_ev_{id}"),
            ts,
            referer: None,
            country: country.map(str::to_string),
            user_agent: None,
            city: None,
            bot: false,
            ip: None,
            fbc: None,
            variant: None,
            tenant_id: 0,
        }
    }

    fn ga4_config() -> PixelConfig {
        PixelConfig {
            id: 1,
            provider: Provider::Ga4,
            credentials: PixelCredentials {
                measurement_id: Some("G-ABC123".into()),
                api_secret: Some("secret1".into()),
                pixel_id: None,
                access_token: None,
            },
            active: true,
            created: 0,
        }
    }

    fn meta_config() -> PixelConfig {
        PixelConfig {
            id: 2,
            provider: Provider::MetaCapi,
            credentials: PixelCredentials {
                measurement_id: None,
                api_secret: None,
                pixel_id: Some("1234567890".into()),
                access_token: Some("token1".into()),
            },
            active: true,
            created: 0,
        }
    }

    #[test]
    fn provider_serializes_lowercase() {
        assert_eq!(serde_json::to_string(&Provider::Ga4).unwrap(), "\"ga4\"");
        assert_eq!(
            serde_json::to_string(&Provider::MetaCapi).unwrap(),
            "\"meta_capi\""
        );
    }

    /// Test-only key: distinct from the crate's default dev key so a test
    /// mixing them up would fail loudly instead of accidentally matching.
    const TEST_KEY: u64 = 0x1234_5678_9ABC_DEF0;

    #[test]
    fn ga4_payload_has_expected_shape_and_batches() {
        let events = vec![
            ev(10, 100, Some("BR")),
            ev(11, 101, Some("US")),
            ev(12, 102, None),
        ];
        let payload = ga4_payload(&events, TEST_KEY);
        assert!(payload["client_id"].is_string());
        let events_arr = payload["events"].as_array().unwrap();
        assert_eq!(events_arr.len(), 3);
        assert_eq!(events_arr[0]["name"], GA4_EVENT_NAME);
        assert_eq!(
            events_arr[0]["params"]["link_code"],
            link_code(10, TEST_KEY)
        );
        assert_eq!(events_arr[0]["params"]["country"], "BR");
        assert_eq!(events_arr[0]["params"]["transaction_id"], "clk_ev_10");
        assert_eq!(
            events_arr[1]["params"]["link_code"],
            link_code(11, TEST_KEY)
        );
        assert_eq!(events_arr[1]["params"]["transaction_id"], "clk_ev_11");
        assert_eq!(events_arr[2]["params"]["country"], Value::Null);
    }

    #[test]
    fn ga4_payload_transaction_id_matches_event_id_and_is_stable() {
        let events = vec![ev(10, 100, Some("BR"))];
        let a = ga4_payload(&events, TEST_KEY);
        let b = ga4_payload(&events, TEST_KEY);
        assert_eq!(a["events"][0]["params"]["transaction_id"], "clk_ev_10");
        assert_eq!(
            a["events"][0]["params"]["transaction_id"],
            events[0].event_id
        );
        assert_eq!(
            a["events"][0]["params"]["transaction_id"],
            b["events"][0]["params"]["transaction_id"]
        );
    }

    #[test]
    fn ga4_payload_link_code_is_the_real_short_code_not_the_internal_id() {
        let events = vec![ev(10, 100, Some("BR"))];
        let payload = ga4_payload(&events, TEST_KEY);
        let code = payload["events"][0]["params"]["link_code"]
            .as_str()
            .unwrap();
        assert_ne!(code, "10");
        assert_eq!(code, codec::to_base62(permute::encode(10, TEST_KEY)));
    }

    #[test]
    fn ga4_payload_client_id_is_stable_across_calls() {
        let events = vec![ev(1, 1, None)];
        let a = ga4_payload(&events, TEST_KEY);
        let b = ga4_payload(&events, TEST_KEY);
        assert_eq!(a["client_id"], b["client_id"]);
    }

    #[test]
    fn meta_payload_has_expected_shape_and_batches() {
        let events = vec![
            ClickEvent {
                id: 20,
                event_id: "clk_ev_20".into(),
                ts: 200,
                referer: None,
                country: Some("BR".into()),
                user_agent: Some("Mozilla/5.0 (iPhone)".into()),
                city: None,
                bot: false,
                ip: Some("203.0.113.5".into()),
                fbc: Some("fb.1.200000.abc".into()),
                variant: None,
                tenant_id: 0,
            },
            ev(21, 201, Some("US")),
        ];
        let payload = meta_payload(&events, TEST_KEY);
        let data = payload["data"].as_array().unwrap();
        assert_eq!(data.len(), 2);
        assert_eq!(data[0]["event_name"], "Lead");
        assert_eq!(data[0]["event_time"], 200);
        assert_eq!(data[0]["action_source"], "website");
        assert_eq!(data[0]["event_id"], "clk_ev_20");
        assert_eq!(data[0]["event_id"], events[0].event_id);
        assert_eq!(data[1]["event_id"], "clk_ev_21");
        assert!(
            data[0]["custom_data"].get("event_id").is_none(),
            "event_id belongs at the event level, not inside custom_data"
        );
        assert!(
            data[0]["user_data"].get("event_id").is_none(),
            "event_id belongs at the event level, not inside user_data"
        );
        assert_eq!(data[0]["custom_data"]["link_code"], link_code(20, TEST_KEY));
        assert_eq!(data[1]["custom_data"]["link_code"], link_code(21, TEST_KEY));

        let ud = &data[0]["user_data"];
        assert_eq!(ud["client_ip_address"], "203.0.113.5");
        assert_eq!(ud["client_user_agent"], "Mozilla/5.0 (iPhone)");
        assert_eq!(ud["fbc"], "fb.1.200000.abc");
        assert_eq!(ud["country"], sha256_hex("BR"));
        assert_eq!(sha256_hex("BR"), sha256_hex("br"));
        assert_ne!(ud["client_user_agent"], sha256_hex("Mozilla/5.0 (iPhone)"));

        let ud1 = &data[1]["user_data"];
        assert!(ud1.get("client_ip_address").is_none());
        assert!(ud1.get("client_user_agent").is_none());
        assert!(ud1.get("fbc").is_none());
        assert_eq!(ud1["country"], sha256_hex("US"));
    }

    #[test]
    fn meta_payload_event_id_matches_event_id_and_is_stable() {
        let events = vec![ev(20, 200, Some("BR"))];
        let a = meta_payload(&events, TEST_KEY);
        let b = meta_payload(&events, TEST_KEY);
        assert_eq!(a["data"][0]["event_id"], "clk_ev_20");
        assert_eq!(a["data"][0]["event_id"], events[0].event_id);
        assert_eq!(a["data"][0]["event_id"], b["data"][0]["event_id"]);
    }

    #[test]
    fn provider_url_ga4() {
        let url = provider_url("https://www.google-analytics.com", &ga4_config()).unwrap();
        assert_eq!(
            url,
            "https://www.google-analytics.com/mp/collect?measurement_id=G-ABC123&api_secret=secret1"
        );
    }

    #[test]
    fn provider_url_meta() {
        let url = provider_url("https://graph.facebook.com", &meta_config()).unwrap();
        assert_eq!(
            url,
            "https://graph.facebook.com/v19.0/1234567890/events?access_token=token1"
        );
    }

    #[test]
    fn provider_url_percent_encodes_credentials_with_special_characters() {
        let mut config = ga4_config();
        config.credentials.api_secret = Some("se&cret#1 two".into());
        let url = provider_url("https://www.google-analytics.com", &config).unwrap();
        // Query values are form-urlencoded: space becomes `+`, not `%20`.
        assert!(url.contains("api_secret=se%26cret%231+two"));
        assert!(!url.contains("se&cret#1 two"));

        let mut config = meta_config();
        config.credentials.pixel_id = Some("id/with slash".into());
        let url = provider_url("https://graph.facebook.com", &config).unwrap();
        assert!(url.contains("id%2Fwith%20slash"));
        assert!(!url.contains("id/with slash"));
    }

    #[test]
    fn provider_url_invalid_base_is_an_error_not_a_panic() {
        let err = provider_url("not a url", &ga4_config()).unwrap_err();
        assert!(matches!(err, PixelError::InvalidBase));

        let err = provider_url("not a url", &meta_config()).unwrap_err();
        assert!(matches!(err, PixelError::InvalidBase));
    }

    type Captured = Arc<Mutex<Vec<(String, String, Value)>>>;

    async fn mock_server(path: &'static str) -> (String, Captured) {
        let captured: Captured = Arc::new(Mutex::new(Vec::new()));
        let state = captured.clone();

        async fn handler(
            State(state): State<Captured>,
            req: axum::extract::Request,
        ) -> axum::http::StatusCode {
            let (parts, body) = req.into_parts();
            let full_path = parts
                .uri
                .path_and_query()
                .map(|pq| pq.as_str().to_string())
                .unwrap_or_default();
            let bytes = axum::body::to_bytes(body, usize::MAX).await.unwrap();
            let json: Value = serde_json::from_slice(&bytes).unwrap();
            state
                .lock()
                .unwrap()
                .push((parts.method.to_string(), full_path, json));
            axum::http::StatusCode::OK
        }

        let app = Router::new().route(path, post(handler)).with_state(state);
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        (format!("http://{addr}"), captured)
    }

    #[tokio::test]
    async fn forward_ga4_posts_to_mp_collect_with_body() {
        let (base, captured) = mock_server("/mp/collect").await;
        let client = reqwest::Client::new();
        let events = vec![ev(5, 50, Some("BR"))];

        forward(&client, &base, &ga4_config(), &events, TEST_KEY)
            .await
            .unwrap();

        let calls = captured.lock().unwrap();
        assert_eq!(calls.len(), 1);
        let (method, path, body) = &calls[0];
        assert_eq!(method, "POST");
        assert!(path.starts_with("/mp/collect?measurement_id=G-ABC123"));
        assert_eq!(
            body["events"][0]["params"]["link_code"],
            link_code(5, TEST_KEY)
        );
    }

    #[tokio::test]
    async fn forward_meta_posts_to_events_path_with_body() {
        let (base, captured) = mock_server("/v19.0/1234567890/events").await;
        let client = reqwest::Client::new();
        let events = vec![ev(6, 60, Some("US"))];

        forward(&client, &base, &meta_config(), &events, TEST_KEY)
            .await
            .unwrap();

        let calls = captured.lock().unwrap();
        assert_eq!(calls.len(), 1);
        let (method, path, body) = &calls[0];
        assert_eq!(method, "POST");
        assert!(path.starts_with("/v19.0/1234567890/events?access_token=token1"));
        assert_eq!(
            body["data"][0]["custom_data"]["link_code"],
            link_code(6, TEST_KEY)
        );
    }

    #[tokio::test]
    async fn forward_empty_batch_is_a_noop() {
        let client = reqwest::Client::new();
        let events: Vec<ClickEvent> = Vec::new();
        forward(
            &client,
            "http://127.0.0.1:1",
            &ga4_config(),
            &events,
            TEST_KEY,
        )
        .await
        .unwrap();
    }
}
