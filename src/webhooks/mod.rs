//! Outbound webhook types and Standard Webhooks (symmetric v1) signing.
//!
//! Signing follows <https://www.standardwebhooks.com/>: the signed string is
//! `"{msg_id}.{timestamp}.{body}"` (literal dots), the key is the base64-decoded
//! secret with the `whsec_` prefix stripped, and the signature is
//! `"v1," + base64(HMAC_SHA256(key, signed_string))`.

pub mod delivery;

use base64::{engine::general_purpose::STANDARD as base64_engine, Engine as _};
use hmac::{Hmac, KeyInit, Mac};
use serde::{Deserialize, Serialize};
use sha2::Sha256;
use std::fmt;

type HmacSha256 = Hmac<Sha256>;

/// Kind of event a webhook subscription can be notified about.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum EventType {
    #[serde(rename = "link.created")]
    LinkCreated,
    #[serde(rename = "link.updated")]
    LinkUpdated,
    #[serde(rename = "link.deleted")]
    LinkDeleted,
    #[serde(rename = "link.expired")]
    LinkExpired,
    #[serde(rename = "link.clicked")]
    LinkClicked,
    #[serde(rename = "link.broken")]
    LinkBroken,
    #[serde(rename = "link.recovered")]
    LinkRecovered,
}

impl EventType {
    /// The wire string for this event type (matches the serde rename).
    pub fn as_str(&self) -> &'static str {
        match self {
            EventType::LinkCreated => "link.created",
            EventType::LinkUpdated => "link.updated",
            EventType::LinkDeleted => "link.deleted",
            EventType::LinkExpired => "link.expired",
            EventType::LinkClicked => "link.clicked",
            EventType::LinkBroken => "link.broken",
            EventType::LinkRecovered => "link.recovered",
        }
    }

    /// Parses the wire string back into an `EventType`, inverse of `as_str`.
    /// Used by the durable relay to reconstruct the event kind from the
    /// `event_type` column persisted in the outbox. Returns `None` on an
    /// unrecognized value.
    pub fn from_wire(s: &str) -> Option<EventType> {
        match s {
            "link.created" => Some(EventType::LinkCreated),
            "link.updated" => Some(EventType::LinkUpdated),
            "link.deleted" => Some(EventType::LinkDeleted),
            "link.expired" => Some(EventType::LinkExpired),
            "link.clicked" => Some(EventType::LinkClicked),
            "link.broken" => Some(EventType::LinkBroken),
            "link.recovered" => Some(EventType::LinkRecovered),
            _ => None,
        }
    }
}

/// Kind of channel a webhook subscription delivers to. `Generic` is a raw,
/// Standard-Webhooks-signed HTTP callback (the #1 behavior); the other
/// variants are native chat integrations whose incoming URL doubles as the
/// authentication secret, so they are delivered unsigned (see
/// `channel_payload` and `delivery::deliver_one`).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum SubscriptionKind {
    #[default]
    #[serde(rename = "generic")]
    Generic,
    #[serde(rename = "slack")]
    Slack,
    #[serde(rename = "discord")]
    Discord,
    #[serde(rename = "telegram")]
    Telegram,
}

impl SubscriptionKind {
    /// The wire string for this kind (matches the serde rename); also used
    /// as the on-disk representation for backends that store `kind` as a
    /// plain text column (see `store::postgres::row_to_webhook`).
    pub fn as_str(&self) -> &'static str {
        match self {
            SubscriptionKind::Generic => "generic",
            SubscriptionKind::Slack => "slack",
            SubscriptionKind::Discord => "discord",
            SubscriptionKind::Telegram => "telegram",
        }
    }

    /// Parses the wire/column string back into a kind. Unrecognized values
    /// fall back to `Generic` rather than erroring, matching the
    /// `#[serde(default)]` behavior on `WebhookSubscription::kind` for
    /// pre-#6 rows that never had this column/field.
    pub fn from_str_or_generic(s: &str) -> Self {
        match s {
            "slack" => SubscriptionKind::Slack,
            "discord" => SubscriptionKind::Discord,
            "telegram" => SubscriptionKind::Telegram,
            _ => SubscriptionKind::Generic,
        }
    }
}

/// A registered outbound webhook subscription.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WebhookSubscription {
    pub id: u64,
    pub url: String,
    pub events: Vec<EventType>,
    pub secret: String,
    pub active: bool,
    pub created: u64,
    /// Channel kind; defaults to `Generic` so pre-#6 persisted blobs (which
    /// never had this field) deserialize unchanged.
    #[serde(default)]
    pub kind: SubscriptionKind,
}

/// A concrete event ready to be delivered: the event kind plus the exact
/// serialized JSON body that gets signed and sent verbatim, plus the tenant
/// that owns it. `tenant_id` is what lets the in-memory worker's
/// per-tenant subscription snapshot (LUC-63) route the event only to that
/// tenant's subscriptions (see `webhooks::delivery::deliver_to_matching`);
/// the durable outbox path (`lifecycle_deliveries`) stamps the same tenant
/// onto its `OutboxRow`s independently and does not read this field.
#[derive(Debug, Clone)]
pub struct WebhookEvent {
    pub event_type: EventType,
    pub body: String,
    pub tenant_id: crate::tenant::TenantId,
}

/// Errors that can occur while signing a webhook payload.
#[derive(Debug, PartialEq, Eq)]
pub enum SignError {
    /// The secret's `whsec_`-stripped remainder is not valid base64.
    InvalidSecretEncoding,
    /// The HMAC key material was rejected (should not happen for HMAC-SHA256,
    /// which accepts keys of any length).
    InvalidKeyLength,
    /// The secret is missing the `whsec_` prefix, or decodes to an empty
    /// key. Either way, signing with it would be a no-op an attacker can
    /// reproduce (an empty HMAC key is a fixed, guessable key); this is a
    /// defensive backstop, since the real fix is that a `Generic`
    /// subscription's secret is never left empty (see `admin_webhooks_create`
    /// / `admin_webhooks_patch`).
    EmptyOrMalformedSecret,
}

impl fmt::Display for SignError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            SignError::InvalidSecretEncoding => write!(f, "secret is not valid base64"),
            SignError::InvalidKeyLength => write!(f, "invalid HMAC key length"),
            SignError::EmptyOrMalformedSecret => {
                write!(
                    f,
                    "secret is missing whsec_ prefix or decodes to an empty key"
                )
            }
        }
    }
}

impl std::error::Error for SignError {}

/// Generates a new webhook signing secret: `whsec_` followed by the base64
/// encoding of 32 cryptographically random bytes.
pub fn generate_secret() -> String {
    let mut bytes = [0u8; 32];
    getrandom::fill(&mut bytes).expect("system RNG must be available");
    format!("whsec_{}", base64_engine.encode(bytes))
}

/// Decodes a base64 string. Exposed for tests that need to validate the
/// shape of a generated secret without depending on a specific base64 crate.
pub fn base64_decode(s: &str) -> Result<Vec<u8>, base64::DecodeError> {
    base64_engine.decode(s)
}

/// Signs a webhook payload following the Standard Webhooks (symmetric v1) scheme.
///
/// `secret` must be of the form `whsec_<base64>`. Returns `"v1,<base64 mac>"`.
pub fn sign(secret: &str, msg_id: &str, timestamp: i64, body: &str) -> Result<String, SignError> {
    let Some(encoded_key) = secret.strip_prefix("whsec_") else {
        return Err(SignError::EmptyOrMalformedSecret);
    };
    let key = base64_engine
        .decode(encoded_key)
        .map_err(|_| SignError::InvalidSecretEncoding)?;
    if key.is_empty() {
        return Err(SignError::EmptyOrMalformedSecret);
    }

    let signed_string = format!("{msg_id}.{timestamp}.{body}");

    let mut mac = HmacSha256::new_from_slice(&key).map_err(|_| SignError::InvalidKeyLength)?;
    mac.update(signed_string.as_bytes());
    let mac_bytes = mac.finalize().into_bytes();

    Ok(format!("v1,{}", base64_engine.encode(mac_bytes)))
}

/// Whether an event of type `ev` should be delivered to `sub`: the
/// subscription must be active and subscribed to that event type.
pub fn matches(sub: &WebhookSubscription, ev: &EventType) -> bool {
    sub.active && sub.events.contains(ev)
}

/// Renders a plain-text (no emoji) chat message for `event_type`, parsing
/// the fields a channel needs (`data.code`/`data.url`, optionally
/// `data.country`) out of the same JSON `body` the generic path signs and
/// sends verbatim. If `body` doesn't parse as JSON, falls back to the bare
/// event type string, since there's nothing else reliable to show.
pub fn format_message(event_type: EventType, body: &str) -> String {
    let Ok(parsed) = serde_json::from_str::<serde_json::Value>(body) else {
        return event_type.as_str().to_string();
    };
    let code = parsed["data"]["code"].as_str().unwrap_or("");
    let url = parsed["data"]["url"].as_str().unwrap_or("");
    match event_type {
        EventType::LinkCreated => format!("New short link: {code} -> {url}"),
        EventType::LinkUpdated => format!("Short link updated: {code} -> {url}"),
        EventType::LinkDeleted => format!("Short link deleted: {code}"),
        EventType::LinkExpired => format!("Short link expired: {code}"),
        EventType::LinkBroken => format!("Short link broken: {code} -> {url}"),
        EventType::LinkRecovered => format!("Short link recovered: {code} -> {url}"),
        EventType::LinkClicked => {
            let mut msg = format!("Click on {code} -> {url}");
            if let Some(country) = parsed["data"]["country"].as_str() {
                msg.push_str(&format!(" ({country})"));
            }
            msg
        }
    }
}

/// Builds the JSON body a chat channel expects for `message`, per `kind`.
/// Returns `None` for `Generic`, which has no channel payload: it signs and
/// sends the original event body verbatim instead (see `delivery::deliver_one`).
pub fn channel_payload(kind: SubscriptionKind, message: &str) -> Option<String> {
    match kind {
        SubscriptionKind::Generic => None,
        SubscriptionKind::Slack | SubscriptionKind::Telegram => {
            Some(serde_json::json!({ "text": message }).to_string())
        }
        SubscriptionKind::Discord => Some(serde_json::json!({ "content": message }).to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn event_type_wire_strings() {
        assert_eq!(EventType::LinkCreated.as_str(), "link.created");
        assert_eq!(
            serde_json::to_string(&EventType::LinkClicked).unwrap(),
            "\"link.clicked\""
        );
    }

    #[test]
    fn health_event_types_round_trip() {
        for (ev, wire) in [
            (EventType::LinkBroken, "link.broken"),
            (EventType::LinkRecovered, "link.recovered"),
        ] {
            assert_eq!(ev.as_str(), wire);
            assert_eq!(EventType::from_wire(wire), Some(ev));
            // serde rename matches the wire string.
            assert_eq!(serde_json::to_string(&ev).unwrap(), format!("\"{wire}\""));
        }
        assert_eq!(EventType::from_wire("link.nonsense"), None);
    }

    /// Standard Webhooks symmetric test vector (from the Svix/Standard Webhooks
    /// reference implementations): secret
    /// `whsec_MfKQ9r8GKYqrTwjUPD8ILPZIo2LaLaSw`, id
    /// `msg_p5jXN8AQM9LWM0D4loKWxJek`, timestamp `1614265330`, payload
    /// `{"test": 2432232314}` (note the literal space after the colon),
    /// expected signature `v1,g0hM9SsE+OTPJTGt/tmIKtSyZlE3uFJELVlNIOLJ1OE=`.
    /// Verified independently outside this crate (Python hmac/hashlib/base64,
    /// recomputed from scratch) that only this exact byte sequence reproduces
    /// the documented signature; a compact `{"test":2432232314}` (no space)
    /// does not.
    #[test]
    fn sign_matches_standard_webhooks_vector() {
        let sig = sign(
            "whsec_MfKQ9r8GKYqrTwjUPD8ILPZIo2LaLaSw",
            "msg_p5jXN8AQM9LWM0D4loKWxJek",
            1614265330,
            "{\"test\": 2432232314}",
        )
        .unwrap();
        assert_eq!(sig, "v1,g0hM9SsE+OTPJTGt/tmIKtSyZlE3uFJELVlNIOLJ1OE=");
    }

    #[test]
    fn generate_secret_shape() {
        let s = generate_secret();
        assert!(s.starts_with("whsec_"));
        assert!(base64_decode(&s["whsec_".len()..]).is_ok());
    }

    #[test]
    fn matches_respects_active_and_event() {
        let sub = WebhookSubscription {
            id: 1,
            url: "https://x".into(),
            events: vec![EventType::LinkCreated],
            secret: "whsec_x".into(),
            active: true,
            created: 0,
            kind: SubscriptionKind::Generic,
        };
        assert!(matches(&sub, &EventType::LinkCreated));
        assert!(!matches(&sub, &EventType::LinkClicked));
        let off = WebhookSubscription {
            active: false,
            ..sub.clone()
        };
        assert!(!matches(&off, &EventType::LinkCreated));
    }

    #[test]
    fn subscription_kind_wire_strings_are_lowercase() {
        assert_eq!(
            serde_json::to_string(&SubscriptionKind::Generic).unwrap(),
            "\"generic\""
        );
        assert_eq!(
            serde_json::to_string(&SubscriptionKind::Slack).unwrap(),
            "\"slack\""
        );
        assert_eq!(
            serde_json::to_string(&SubscriptionKind::Discord).unwrap(),
            "\"discord\""
        );
        assert_eq!(
            serde_json::to_string(&SubscriptionKind::Telegram).unwrap(),
            "\"telegram\""
        );
    }

    /// Regression: a pre-#6 persisted `WebhookSubscription` blob has no
    /// `kind` field at all. `#[serde(default)]` must fill it with `Generic`
    /// rather than failing to deserialize.
    #[test]
    fn subscription_without_kind_field_deserializes_as_generic() {
        let blob = r#"{
            "id": 1,
            "url": "https://x",
            "events": ["link.created"],
            "secret": "whsec_x",
            "active": true,
            "created": 0
        }"#;
        let sub: WebhookSubscription = serde_json::from_str(blob).unwrap();
        assert_eq!(sub.kind, SubscriptionKind::Generic);
    }

    #[test]
    fn format_message_created() {
        let body = r#"{"type":"link.created","data":{"code":"abc123","url":"https://e.com"}}"#;
        assert_eq!(
            format_message(EventType::LinkCreated, body),
            "New short link: abc123 -> https://e.com"
        );
    }

    #[test]
    fn format_message_updated() {
        let body = r#"{"type":"link.updated","data":{"code":"abc123","url":"https://e.com"}}"#;
        assert_eq!(
            format_message(EventType::LinkUpdated, body),
            "Short link updated: abc123 -> https://e.com"
        );
    }

    #[test]
    fn format_message_deleted() {
        let body = r#"{"type":"link.deleted","data":{"code":"abc123"}}"#;
        assert_eq!(
            format_message(EventType::LinkDeleted, body),
            "Short link deleted: abc123"
        );
    }

    #[test]
    fn format_message_expired() {
        let body = r#"{"type":"link.expired","data":{"code":"abc123"}}"#;
        assert_eq!(
            format_message(EventType::LinkExpired, body),
            "Short link expired: abc123"
        );
    }

    #[test]
    fn format_message_clicked_without_country() {
        let body = r#"{"type":"link.clicked","data":{"code":"abc123","url":"https://e.com"}}"#;
        assert_eq!(
            format_message(EventType::LinkClicked, body),
            "Click on abc123 -> https://e.com"
        );
    }

    #[test]
    fn format_message_clicked_with_country() {
        let body = r#"{"type":"link.clicked","data":{"code":"abc123","url":"https://e.com","country":"BR"}}"#;
        assert_eq!(
            format_message(EventType::LinkClicked, body),
            "Click on abc123 -> https://e.com (BR)"
        );
    }

    #[test]
    fn format_message_falls_back_to_event_type_on_parse_failure() {
        assert_eq!(
            format_message(EventType::LinkCreated, "not json"),
            "link.created"
        );
    }

    #[test]
    fn channel_payload_slack_and_telegram_use_text_field() {
        assert_eq!(
            channel_payload(SubscriptionKind::Slack, "hello"),
            Some(r#"{"text":"hello"}"#.to_string())
        );
        assert_eq!(
            channel_payload(SubscriptionKind::Telegram, "hello"),
            Some(r#"{"text":"hello"}"#.to_string())
        );
    }

    #[test]
    fn channel_payload_discord_uses_content_field() {
        assert_eq!(
            channel_payload(SubscriptionKind::Discord, "hello"),
            Some(r#"{"content":"hello"}"#.to_string())
        );
    }

    #[test]
    fn channel_payload_generic_is_none() {
        assert_eq!(channel_payload(SubscriptionKind::Generic, "hello"), None);
    }
}
