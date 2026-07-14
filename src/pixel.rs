//! Server-side conversion forwarding (roadmap #14): pixel config type,
//! provider payload formatters (GA4 Measurement Protocol, Meta Conversions
//! API) and an injectable-base forwarder. Wiring into the analytics worker
//! and the admin endpoints is a follow-up task; this module is pure/mockable
//! on its own.

use crate::analytics::ClickEvent;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::sync::OnceLock;
use std::time::Duration;

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
}

impl std::fmt::Display for PixelError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            PixelError::Http(e) => write!(f, "http: {e}"),
            PixelError::Status(s) => write!(f, "provider returned status {s}"),
        }
    }
}
impl std::error::Error for PixelError {}

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

/// Builds the GA4 Measurement Protocol batch body for a slice of events.
/// `client_id` is synthetic (per-instance), never a real user id.
pub fn ga4_payload(events: &[ClickEvent]) -> Value {
    let events_json: Vec<Value> = events
        .iter()
        .map(|e| {
            json!({
                "name": GA4_EVENT_NAME,
                "params": {
                    "link_code": e.id.to_string(),
                    "country": e.country,
                },
            })
        })
        .collect();
    json!({
        "client_id": instance_client_id(),
        "events": events_json,
    })
}

/// Builds the Meta Conversions API batch body for a slice of events.
pub fn meta_payload(events: &[ClickEvent]) -> Value {
    let data: Vec<Value> = events
        .iter()
        .map(|e| {
            json!({
                "event_name": META_EVENT_NAME,
                "event_time": e.ts,
                "action_source": "website",
                "custom_data": {
                    "link_code": e.id.to_string(),
                },
            })
        })
        .collect();
    json!({ "data": data })
}

/// Builds the provider URL. `base` is injectable so tests can point at a
/// local mock server; production always passes the fixed provider host
/// (no SSRF surface: the operator supplies credentials, not URLs).
pub fn provider_url(base: &str, config: &PixelConfig) -> String {
    match config.provider {
        Provider::Ga4 => format!(
            "{base}/mp/collect?measurement_id={}&api_secret={}",
            config.credentials.measurement_id.as_deref().unwrap_or(""),
            config.credentials.api_secret.as_deref().unwrap_or(""),
        ),
        Provider::MetaCapi => format!(
            "{base}/v19.0/{}/events?access_token={}",
            config.credentials.pixel_id.as_deref().unwrap_or(""),
            config.credentials.access_token.as_deref().unwrap_or(""),
        ),
    }
}

/// Forwards a batch of click events to a single pixel config. Async only:
/// callers must run this off the redirect hot path (the analytics worker).
/// Fails open at the caller: an `Err` here must never affect a redirect.
pub async fn forward(
    client: &reqwest::Client,
    base: &str,
    config: &PixelConfig,
    events: &[ClickEvent],
) -> Result<(), PixelError> {
    if events.is_empty() {
        return Ok(());
    }
    let payload = match config.provider {
        Provider::Ga4 => ga4_payload(events),
        Provider::MetaCapi => meta_payload(events),
    };
    let url = provider_url(base, config);
    let resp = client
        .post(url)
        .json(&payload)
        .timeout(FORWARD_TIMEOUT)
        .send()
        .await
        .map_err(PixelError::Http)?;
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
            ts,
            referer: None,
            country: country.map(str::to_string),
            user_agent: None,
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

    #[test]
    fn ga4_payload_has_expected_shape_and_batches() {
        let events = vec![
            ev(10, 100, Some("BR")),
            ev(11, 101, Some("US")),
            ev(12, 102, None),
        ];
        let payload = ga4_payload(&events);
        assert!(payload["client_id"].is_string());
        let events_arr = payload["events"].as_array().unwrap();
        assert_eq!(events_arr.len(), 3);
        assert_eq!(events_arr[0]["name"], GA4_EVENT_NAME);
        assert_eq!(events_arr[0]["params"]["link_code"], "10");
        assert_eq!(events_arr[0]["params"]["country"], "BR");
        assert_eq!(events_arr[1]["params"]["link_code"], "11");
        assert_eq!(events_arr[2]["params"]["country"], Value::Null);
    }

    #[test]
    fn ga4_payload_client_id_is_stable_across_calls() {
        let events = vec![ev(1, 1, None)];
        let a = ga4_payload(&events);
        let b = ga4_payload(&events);
        assert_eq!(a["client_id"], b["client_id"]);
    }

    #[test]
    fn meta_payload_has_expected_shape_and_batches() {
        let events = vec![ev(20, 200, Some("BR")), ev(21, 201, Some("US"))];
        let payload = meta_payload(&events);
        let data = payload["data"].as_array().unwrap();
        assert_eq!(data.len(), 2);
        assert_eq!(data[0]["event_name"], "Lead");
        assert_eq!(data[0]["event_time"], 200);
        assert_eq!(data[0]["action_source"], "website");
        assert_eq!(data[0]["custom_data"]["link_code"], "20");
        assert_eq!(data[1]["custom_data"]["link_code"], "21");
    }

    #[test]
    fn provider_url_ga4() {
        let url = provider_url("https://www.google-analytics.com", &ga4_config());
        assert_eq!(
            url,
            "https://www.google-analytics.com/mp/collect?measurement_id=G-ABC123&api_secret=secret1"
        );
    }

    #[test]
    fn provider_url_meta() {
        let url = provider_url("https://graph.facebook.com", &meta_config());
        assert_eq!(
            url,
            "https://graph.facebook.com/v19.0/1234567890/events?access_token=token1"
        );
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

        forward(&client, &base, &ga4_config(), &events)
            .await
            .unwrap();

        let calls = captured.lock().unwrap();
        assert_eq!(calls.len(), 1);
        let (method, path, body) = &calls[0];
        assert_eq!(method, "POST");
        assert!(path.starts_with("/mp/collect?measurement_id=G-ABC123"));
        assert_eq!(body["events"][0]["params"]["link_code"], "5");
    }

    #[tokio::test]
    async fn forward_meta_posts_to_events_path_with_body() {
        let (base, captured) = mock_server("/v19.0/1234567890/events").await;
        let client = reqwest::Client::new();
        let events = vec![ev(6, 60, Some("US"))];

        forward(&client, &base, &meta_config(), &events)
            .await
            .unwrap();

        let calls = captured.lock().unwrap();
        assert_eq!(calls.len(), 1);
        let (method, path, body) = &calls[0];
        assert_eq!(method, "POST");
        assert!(path.starts_with("/v19.0/1234567890/events?access_token=token1"));
        assert_eq!(body["data"][0]["custom_data"]["link_code"], "6");
    }

    #[tokio::test]
    async fn forward_empty_batch_is_a_noop() {
        let client = reqwest::Client::new();
        let events: Vec<ClickEvent> = Vec::new();
        forward(&client, "http://127.0.0.1:1", &ga4_config(), &events)
            .await
            .unwrap();
    }
}
