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
    #[serde(rename = "link.threshold_reached")]
    LinkThresholdReached,
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
            EventType::LinkThresholdReached => "link.threshold_reached",
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
            "link.threshold_reached" => Some(EventType::LinkThresholdReached),
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

/// Placeholder printed when the destination has no host to show, either
/// because the string is not a URL at all or because the authority part is
/// empty. Never contains anything derived from the value itself.
const NO_HOST: &str = "<invalid url>";

/// A webhook destination URL. For Slack, Discord, Telegram and most of the
/// generic connectors (Make, Zapier, n8n) **the URL is the credential**: the
/// token lives in the path, and whoever holds the URL can post to the channel.
/// That is why `Display` and `Debug` are redacted: any
/// `tracing::warn!(url = %sub.url, ...)` prints the host and nothing else.
///
/// The raw value only comes out of `expose()`, mirroring `ExposeSecret` from
/// `secrecy`. `reqwest` does not implement `IntoUrl` for this type, so every
/// site that builds the outbound request needs an explicit `expose()` and
/// reintroducing the leak becomes a compile error instead of a review habit.
///
/// `serde` is transparent: the value persisted in LMDB and Postgres, and the
/// one returned by `GET /admin/webhooks`, stay the raw string. There is no data
/// migration and no API contract change.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct WebhookUrl(String);

impl WebhookUrl {
    pub fn new(raw: impl Into<String>) -> Self {
        Self(raw.into())
    }

    /// The raw value. Use only where the URL really has to go on the wire.
    pub fn expose(&self) -> &str {
        &self.0
    }

    /// The destination host, with the port when there is one, and nothing
    /// else. `None` when the value does not parse as a URL or has no host.
    ///
    /// Parsed with `url::Url`, the SAME parser that decides where the request
    /// actually goes: `reqwest` uses it through `IntoUrl` and
    /// `abuse::extract_host` uses it to validate the destination. Slicing the
    /// authority by hand diverges from it, and the divergence leaks: the `url`
    /// crate follows WHATWG and treats `\` as `/` in special schemes, so
    /// `https://hooks.example.com\services/T000/SECRET` passes validation, is
    /// delivered with the backslash normalized, and a split on `['/', '?',
    /// '#']` finds no separator and prints the token.
    ///
    /// It allocates, which is fine: nothing here runs on the redirect hot
    /// path. `WebhookDispatcher::try_emit` hands the event to the worker with
    /// `try_send`, and every site that formats a `WebhookUrl` is inside that
    /// worker or the relay.
    fn host_and_port(&self) -> Option<String> {
        let parsed = url::Url::parse(&self.0).ok()?;
        // `Host`'s `Display` brackets IPv6 literals, so `[::1]:8443` comes
        // back the way it went in.
        let host = parsed.host()?;
        Some(match parsed.port() {
            Some(port) => format!("{host}:{port}"),
            None => host.to_string(),
        })
    }
}

impl std::fmt::Display for WebhookUrl {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self.host_and_port() {
            Some(host) => write!(f, "{host}/\u{2026}"),
            // Nothing derived from the value: a string that does not parse is
            // exactly the case where the "host" would be the credential.
            None => f.write_str(NO_HOST),
        }
    }
}

impl std::fmt::Debug for WebhookUrl {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "WebhookUrl({self})")
    }
}

impl From<String> for WebhookUrl {
    fn from(s: String) -> Self {
        Self(s)
    }
}

impl From<&str> for WebhookUrl {
    fn from(s: &str) -> Self {
        Self(s.to_string())
    }
}

/// A registered outbound webhook subscription.
///
/// `Debug` is written by hand (see below) because the derive printed `secret`,
/// the Standard Webhooks signing key, in full. The URL got a newtype instead
/// because it is passed to `reqwest` from several call sites and the newtype
/// turns each of them into an explicit `expose()`; the secret has a single
/// consumer (`sign`), so its only leak surface was `Debug` and closing that one
/// needs no change to serde, to the persisted blob, or to the store shape.
#[derive(Clone, Serialize, Deserialize)]
pub struct WebhookSubscription {
    pub id: u64,
    pub url: WebhookUrl,
    pub events: Vec<EventType>,
    pub secret: String,
    pub active: bool,
    pub created: u64,
    /// Channel kind; defaults to `Generic` so pre-#6 persisted blobs (which
    /// never had this field) deserialize unchanged.
    #[serde(default)]
    pub kind: SubscriptionKind,
    /// Optional human label for the destination, used by the panel to identify
    /// a connection whose URL is otherwise opaque (e.g. the Slack channel name
    /// `#general` captured from the OAuth install). `None` for manually-entered
    /// webhooks and pre-existing rows.
    #[serde(default)]
    pub label: Option<String>,
    /// Id do conector do catalogo (`"zapier"`, `"make"`, `"n8n"`, `"slack"`...).
    /// Desambigua os webhooks genericos, que compartilham `kind: Generic`.
    /// `None` em linhas anteriores a fase 3.
    #[serde(default)]
    pub connector_id: Option<String>,
    /// Id estavel do destino do lado do provedor (o Slack usa o `channel_id`),
    /// para dedup a prova de rename. Generico de proposito para reuso futuro.
    #[serde(default)]
    pub external_id: Option<String>,
    /// Timestamp (epoch secs) da ultima tentativa de entrega registrada.
    #[serde(default)]
    pub last_delivery_at: Option<u64>,
    /// Resultado da ultima entrega registrada (health passivo).
    #[serde(default)]
    pub last_delivery_status: crate::health::HealthStatus,
    /// Why the system disabled this subscription; `None` when `active` reflects
    /// a user choice. Set when a destination answers permanently (404/410) and
    /// the confirmation attempt fails too. This is what lets the panel tell "I
    /// paused it" apart from "the system disabled it": deriving that from
    /// `!active && status == error` would lie when the user manually pauses a
    /// webhook that was already failing.
    #[serde(default)]
    pub disabled_reason: Option<String>,
}

impl std::fmt::Debug for WebhookSubscription {
    /// Mirrors the derive field for field, except `secret`, which is replaced
    /// by a fixed marker. Whether the field is set is still visible, since an
    /// empty secret on a `Generic` subscription is a real misconfiguration
    /// (see `SignError::EmptyOrMalformedSecret`); its value is not.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("WebhookSubscription")
            .field("id", &self.id)
            .field("url", &self.url)
            .field("events", &self.events)
            .field(
                "secret",
                &if self.secret.is_empty() {
                    "<empty>"
                } else {
                    "<redacted>"
                },
            )
            .field("active", &self.active)
            .field("created", &self.created)
            .field("kind", &self.kind)
            .field("label", &self.label)
            .field("connector_id", &self.connector_id)
            .field("external_id", &self.external_id)
            .field("last_delivery_at", &self.last_delivery_at)
            .field("last_delivery_status", &self.last_delivery_status)
            .field("disabled_reason", &self.disabled_reason)
            .finish()
    }
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
#[non_exhaustive]
#[derive(Debug, PartialEq, Eq, thiserror::Error)]
pub enum SignError {
    /// The secret's `whsec_`-stripped remainder is not valid base64.
    #[error("secret is not valid base64")]
    InvalidSecretEncoding,
    /// The HMAC key material was rejected (should not happen for HMAC-SHA256,
    /// which accepts keys of any length).
    #[error("invalid HMAC key length")]
    InvalidKeyLength,
    /// The secret is missing the `whsec_` prefix, or decodes to an empty
    /// key. Either way, signing with it would be a no-op an attacker can
    /// reproduce (an empty HMAC key is a fixed, guessable key); this is a
    /// defensive backstop, since the real fix is that a `Generic`
    /// subscription's secret is never left empty (see `admin_webhooks_create`
    /// / `admin_webhooks_patch`).
    #[error("secret is missing whsec_ prefix or decodes to an empty key")]
    EmptyOrMalformedSecret,
}

/// Generates a new webhook signing secret: `whsec_` followed by the base64
/// encoding of 32 cryptographically random bytes.
pub fn generate_secret() -> String {
    let mut bytes = [0u8; 32];
    #[expect(
        clippy::expect_used,
        reason = "the OS RNG being unavailable is not a recoverable condition for a security path"
    )]
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
        // The cause is dropped on purpose: base64's error names the offending
        // byte and its offset, which is positional information about a secret.
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
        EventType::LinkThresholdReached => {
            let count = parsed["data"]["count"].as_u64().unwrap_or(0);
            let window_secs = parsed["data"]["window_secs"].as_u64().unwrap_or(0);
            format!("Click threshold reached for {code}: {count} clicks in {window_secs}s")
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

    /// A Discord incoming webhook: everything after the id is the credential.
    const DISCORD: &str =
        "https://discord.com/api/webhooks/1234567890/aVerySecretTokenThatMustNeverLeak";

    #[test]
    fn display_redacts_the_token_and_keeps_the_host() {
        let url = WebhookUrl::new(DISCORD);
        let shown = format!("{url}");
        assert!(
            !shown.contains("aVerySecretTokenThatMustNeverLeak"),
            "leaked: {shown}"
        );
        assert!(
            shown.contains("discord.com"),
            "no host left to diagnose with: {shown}"
        );
    }

    #[test]
    fn debug_redacts_the_token_too() {
        let url = WebhookUrl::new(DISCORD);
        let shown = format!("{url:?}");
        assert!(
            !shown.contains("aVerySecretTokenThatMustNeverLeak"),
            "leaked: {shown}"
        );
    }

    /// A subscription is `Debug`, and `{:?}` on the whole struct must not be a
    /// way around the newtype.
    #[test]
    fn subscription_debug_does_not_leak_the_url() {
        let sub = WebhookSubscription {
            id: 1,
            url: WebhookUrl::new(DISCORD),
            events: vec![EventType::LinkCreated],
            secret: "whsec_x".into(),
            active: true,
            created: 0,
            kind: SubscriptionKind::Generic,
            label: None,
            connector_id: None,
            external_id: None,
            last_delivery_at: None,
            last_delivery_status: Default::default(),
            disabled_reason: None,
        };
        let shown = format!("{sub:?}");
        assert!(
            !shown.contains("aVerySecretTokenThatMustNeverLeak"),
            "leaked: {shown}"
        );
    }

    #[test]
    fn userinfo_is_not_printed_either() {
        let url = WebhookUrl::new("https://user:hunter2@hooks.example.com/services/abc");
        let shown = format!("{url}");
        assert!(!shown.contains("hunter2"), "leaked credentials: {shown}");
        assert!(shown.contains("hooks.example.com"), "no host: {shown}");
    }

    #[test]
    fn a_query_string_is_not_printed_either() {
        let url = WebhookUrl::new("https://hooks.example.com?token=hunter2");
        let shown = format!("{url}");
        assert!(!shown.contains("hunter2"), "leaked query: {shown}");
    }

    #[test]
    fn expose_returns_the_raw_url() {
        assert_eq!(WebhookUrl::new(DISCORD).expose(), DISCORD);
    }

    #[test]
    fn serde_is_transparent_so_persisted_blobs_do_not_change() {
        let url = WebhookUrl::new(DISCORD);
        assert_eq!(
            serde_json::to_string(&url).unwrap(),
            format!("\"{DISCORD}\"")
        );
        let back: WebhookUrl = serde_json::from_str(&format!("\"{DISCORD}\"")).unwrap();
        assert_eq!(back.expose(), DISCORD);
    }

    #[test]
    fn legacy_subscription_blob_still_deserializes() {
        let legacy = r#"{"id":7,"url":"https://h/x","events":["link.created"],"secret":"s","active":true,"created":0}"#;
        let sub: WebhookSubscription = serde_json::from_str(legacy).unwrap();
        assert_eq!(sub.url.expose(), "https://h/x");
    }

    #[test]
    fn a_url_without_a_scheme_still_hides_the_path() {
        let url = WebhookUrl::new("not a url at all/aVerySecretTokenThatMustNeverLeak");
        let shown = format!("{url}");
        assert!(
            !shown.contains("aVerySecretTokenThatMustNeverLeak"),
            "leaked: {shown}"
        );
    }

    /// The `url` crate follows WHATWG and treats `\` as `/` in special
    /// schemes, so this value passes `validate_webhook_url` (its
    /// `extract_host` returns `hooks.example.com`) and `reqwest` delivers it
    /// with the backslash normalized to a slash. A hand-rolled split on
    /// `['/', '?', '#']` does not know that and printed the whole token.
    #[test]
    fn a_backslash_authority_does_not_leak_the_path() {
        let raw = "https://hooks.example.com\\services/T000/B000/SUPERSECRETTOKEN";
        // The parser that decides where the request goes agrees on the host.
        assert_eq!(
            crate::abuse::extract_host(raw).as_deref(),
            Some("hooks.example.com")
        );
        let shown = format!("{}", WebhookUrl::new(raw));
        assert!(!shown.contains("SUPERSECRETTOKEN"), "leaked: {shown}");
        assert_eq!(shown, "hooks.example.com/\u{2026}");
    }

    /// A percent-encoded slash in the authority does not parse as a URL at
    /// all, which is exactly the value that reaches the "destination url is
    /// invalid" warn in `delivery.rs`. Reachable through a legacy LMDB blob or
    /// an import, not through `POST /admin/webhooks`.
    #[test]
    fn a_percent_encoded_authority_prints_only_the_placeholder() {
        let raw = "https://host.example%2FSECRETTOK";
        assert_eq!(crate::abuse::extract_host(raw), None);
        let shown = format!("{}", WebhookUrl::new(raw));
        assert!(!shown.contains("SECRETTOK"), "leaked: {shown}");
        assert_eq!(shown, NO_HOST);
    }

    /// A value with no scheme and none of `/?#` has no authority to cut on, so
    /// the hand-rolled slicing printed it whole.
    #[test]
    fn a_bare_token_prints_only_the_placeholder() {
        let shown = format!("{}", WebhookUrl::new("SECRETTOK"));
        assert!(!shown.contains("SECRETTOK"), "leaked: {shown}");
        assert_eq!(shown, NO_HOST);
    }

    #[test]
    fn a_fragment_is_not_printed_either() {
        let url = WebhookUrl::new("https://hooks.example.com#hunter2");
        let shown = format!("{url}");
        assert!(!shown.contains("hunter2"), "leaked fragment: {shown}");
        assert!(shown.contains("hooks.example.com"), "no host: {shown}");
    }

    #[test]
    fn an_ipv6_host_keeps_its_brackets_and_port() {
        let url = WebhookUrl::new("https://[2001:db8::1]:8443/hook/SECRETTOK");
        let shown = format!("{url}");
        assert!(!shown.contains("SECRETTOK"), "leaked: {shown}");
        assert_eq!(shown, "[2001:db8::1]:8443/\u{2026}");
    }

    #[test]
    fn an_empty_value_prints_only_the_placeholder() {
        assert_eq!(format!("{}", WebhookUrl::new("")), NO_HOST);
    }

    /// The signing secret is the other credential on this struct, and `{:?}`
    /// on the whole subscription must not be a way to print it.
    #[test]
    fn subscription_debug_does_not_leak_the_secret() {
        let sub = WebhookSubscription {
            id: 1,
            url: WebhookUrl::new(DISCORD),
            events: vec![EventType::LinkCreated],
            secret: "whsec_aVerySecretSigningKey".into(),
            active: true,
            created: 0,
            kind: SubscriptionKind::Generic,
            label: None,
            connector_id: None,
            external_id: None,
            last_delivery_at: None,
            last_delivery_status: Default::default(),
            disabled_reason: None,
        };
        let shown = format!("{sub:?}");
        assert!(
            !shown.contains("aVerySecretSigningKey"),
            "leaked the signing secret: {shown}"
        );
        // The rest of the struct is still useful for diagnosis.
        assert!(shown.contains("id: 1"), "not diagnosable anymore: {shown}");
    }

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

    #[test]
    fn threshold_reached_event_type_round_trips() {
        assert_eq!(
            EventType::LinkThresholdReached.as_str(),
            "link.threshold_reached"
        );
        assert_eq!(
            EventType::from_wire("link.threshold_reached"),
            Some(EventType::LinkThresholdReached)
        );
        assert_eq!(
            serde_json::to_string(&EventType::LinkThresholdReached).unwrap(),
            "\"link.threshold_reached\""
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
            kind: SubscriptionKind::Generic,
            label: None,
            connector_id: None,
            external_id: None,
            last_delivery_at: None,
            last_delivery_status: Default::default(),
            disabled_reason: None,
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

    #[test]
    fn legacy_json_without_phase3_fields_deserializes_with_defaults() {
        // Um blob gravado antes da fase 3 (sem connector_id/external_id/health).
        let legacy = r#"{"id":7,"url":"https://h/x","events":["link.created"],
            "secret":"","active":true,"created":100,"kind":"generic"}"#;
        let sub: WebhookSubscription = serde_json::from_str(legacy).unwrap();
        assert_eq!(sub.connector_id, None);
        assert_eq!(sub.external_id, None);
        assert_eq!(sub.last_delivery_at, None);
        assert_eq!(sub.last_delivery_status, crate::health::HealthStatus::Never);
    }
}
