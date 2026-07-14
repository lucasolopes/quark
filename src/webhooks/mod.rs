//! Outbound webhook types and Standard Webhooks (symmetric v1) signing.
//!
//! Signing follows <https://www.standardwebhooks.com/>: the signed string is
//! `"{msg_id}.{timestamp}.{body}"` (literal dots), the key is the base64-decoded
//! secret with the `whsec_` prefix stripped, and the signature is
//! `"v1," + base64(HMAC_SHA256(key, signed_string))`.

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
}

/// A concrete event ready to be delivered: the event kind plus the exact
/// serialized JSON body that gets signed and sent verbatim.
#[derive(Debug, Clone)]
pub struct WebhookEvent {
    pub event_type: EventType,
    pub body: String,
}

/// Errors that can occur while signing a webhook payload.
#[derive(Debug, PartialEq, Eq)]
pub enum SignError {
    /// The secret's `whsec_`-stripped remainder is not valid base64.
    InvalidSecretEncoding,
    /// The HMAC key material was rejected (should not happen for HMAC-SHA256,
    /// which accepts keys of any length).
    InvalidKeyLength,
}

impl fmt::Display for SignError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            SignError::InvalidSecretEncoding => write!(f, "secret is not valid base64"),
            SignError::InvalidKeyLength => write!(f, "invalid HMAC key length"),
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
    let encoded_key = secret.strip_prefix("whsec_").unwrap_or(secret);
    let key = base64_engine
        .decode(encoded_key)
        .map_err(|_| SignError::InvalidSecretEncoding)?;

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
        };
        assert!(matches(&sub, &EventType::LinkCreated));
        assert!(!matches(&sub, &EventType::LinkClicked));
        let off = WebhookSubscription {
            active: false,
            ..sub.clone()
        };
        assert!(!matches(&off, &EventType::LinkCreated));
    }
}
