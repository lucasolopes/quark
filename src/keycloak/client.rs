//! HTTP implementation of [`KeycloakAdmin`] against the Keycloak Admin REST
//! API. Mirrors `sheets::client`'s shape (own `reqwest::Client` builder,
//! `.bearer_auth`, `.json`, explicit status checks). Not unit-tested against a
//! live server — there is no Keycloak in this environment; the trait contract
//! is covered by `MockKeycloakAdmin` in `mod.rs`. Kept thin on purpose: the
//! provisioning *flow* (what calls these methods, in what order, on tenant
//! creation) is Task 2, not this module.

use super::{KcError, KeycloakAdmin, KeycloakConfig, SmtpConfig};
use async_trait::async_trait;
use reqwest::StatusCode;
use serde_json::json;
use std::sync::Mutex;
use std::time::{Duration, Instant};

/// Builds the HTTP client for Keycloak admin calls with a request timeout, so
/// a stalled connection cannot hang tenant provisioning forever.
pub fn keycloak_client() -> reqwest::Client {
    reqwest::Client::builder()
        .timeout(Duration::from_secs(30))
        .build()
        .expect("reqwest client builds")
}

struct CachedToken {
    token: String,
    expires_at: Instant,
}

/// The real implementation over `reqwest`. `base`/`admin_client_id`/
/// `admin_client_secret` come from [`KeycloakConfig`]; the admin token is a
/// `client_credentials` grant against the `master` realm, cached until
/// shortly before its reported expiry and refetched on a `401`.
pub struct HttpKeycloakAdmin {
    base: String,
    client: reqwest::Client,
    admin_client_id: String,
    admin_client_secret: String,
    smtp: SmtpConfig,
    login_theme: Option<String>,
    token: Mutex<Option<CachedToken>>,
}

impl HttpKeycloakAdmin {
    pub fn new(cfg: KeycloakConfig, client: reqwest::Client) -> Self {
        HttpKeycloakAdmin {
            base: cfg.base_url,
            client,
            admin_client_id: cfg.admin_client_id,
            admin_client_secret: cfg.admin_client_secret,
            smtp: cfg.smtp,
            login_theme: cfg.login_theme,
            token: Mutex::new(None),
        }
    }

    /// Returns a cached admin token if it has not yet reached its early-expiry
    /// mark, otherwise fetches a fresh one.
    async fn admin_token(&self) -> Result<String, KcError> {
        let cached = self
            .token
            .lock()
            .expect("token mutex")
            .as_ref()
            .filter(|c| c.expires_at > Instant::now())
            .map(|c| c.token.clone());
        match cached {
            Some(t) => Ok(t),
            None => self.fetch_token().await,
        }
    }

    /// Unconditionally fetches a fresh token and replaces the cache. Used both
    /// for the first fetch and to recover from a `401` on a cached token.
    async fn fetch_token(&self) -> Result<String, KcError> {
        let url = format!("{}/realms/master/protocol/openid-connect/token", self.base);
        let resp = self
            .client
            .post(&url)
            .form(&[
                ("grant_type", "client_credentials"),
                ("client_id", self.admin_client_id.as_str()),
                ("client_secret", self.admin_client_secret.as_str()),
            ])
            .send()
            .await
            .map_err(|e| KcError(e.to_string()))?;
        if !resp.status().is_success() {
            return Err(KcError(format!(
                "keycloak admin token request failed: {}",
                resp.status()
            )));
        }
        let body: serde_json::Value = resp.json().await.map_err(|e| KcError(e.to_string()))?;
        let token = body
            .get("access_token")
            .and_then(|v| v.as_str())
            .ok_or_else(|| KcError("admin token response had no access_token".to_string()))?
            .to_string();
        let ttl_secs = body
            .get("expires_in")
            .and_then(|v| v.as_u64())
            .unwrap_or(60);
        // Expire the cache a few seconds early so a request never races the
        // token's real expiry mid-flight.
        let expires_at = Instant::now() + Duration::from_secs(ttl_secs.saturating_sub(5).max(1));
        *self.token.lock().expect("token mutex") = Some(CachedToken {
            token: token.clone(),
            expires_at,
        });
        Ok(token)
    }

    /// POSTs `body` to `url` with a bearer admin token, retrying once with a
    /// freshly fetched token on `401` or `403`. The `403` retry matters right
    /// after `ensure_realm` creates a realm: Keycloak adds that realm's
    /// `<realm>-realm` management roles to the `admin` composite on creation,
    /// but a token minted before the realm existed does not carry them, so the
    /// follow-up `ensure_client`/`ensure_mapper` POST would `403` until the
    /// token is refetched. `409 Conflict` (the resource already exists) is
    /// treated as success, since every provisioning step must be safe to re-run.
    async fn admin_post_idempotent(
        &self,
        url: &str,
        body: &serde_json::Value,
    ) -> Result<(), KcError> {
        let token = self.admin_token().await?;
        let resp = self.post_json(url, &token, body).await?;
        if matches!(
            resp.status(),
            StatusCode::UNAUTHORIZED | StatusCode::FORBIDDEN
        ) {
            let token = self.fetch_token().await?;
            let resp = self.post_json(url, &token, body).await?;
            return Self::ok_or_conflict(resp).await;
        }
        Self::ok_or_conflict(resp).await
    }

    async fn post_json(
        &self,
        url: &str,
        token: &str,
        body: &serde_json::Value,
    ) -> Result<reqwest::Response, KcError> {
        self.client
            .post(url)
            .bearer_auth(token)
            .json(body)
            .send()
            .await
            .map_err(|e| KcError(e.to_string()))
    }

    async fn ok_or_conflict(resp: reqwest::Response) -> Result<(), KcError> {
        if resp.status().is_success() || resp.status() == StatusCode::CONFLICT {
            return Ok(());
        }
        Err(KcError(format!(
            "keycloak request failed: {}",
            resp.status()
        )))
    }

    /// GETs `url` with a bearer admin token, retrying once with a fresh token
    /// on `401` or `403` (see `admin_post_idempotent` for why `403`).
    async fn admin_get(&self, url: &str) -> Result<serde_json::Value, KcError> {
        let token = self.admin_token().await?;
        let mut resp = self
            .client
            .get(url)
            .bearer_auth(&token)
            .send()
            .await
            .map_err(|e| KcError(e.to_string()))?;
        if matches!(
            resp.status(),
            StatusCode::UNAUTHORIZED | StatusCode::FORBIDDEN
        ) {
            let token = self.fetch_token().await?;
            resp = self
                .client
                .get(url)
                .bearer_auth(&token)
                .send()
                .await
                .map_err(|e| KcError(e.to_string()))?;
        }
        if !resp.status().is_success() {
            return Err(KcError(format!(
                "keycloak request failed: {}",
                resp.status()
            )));
        }
        resp.json().await.map_err(|e| KcError(e.to_string()))
    }
}

#[async_trait]
impl KeycloakAdmin for HttpKeycloakAdmin {
    async fn ensure_realm(&self, slug: &str) -> Result<(), KcError> {
        let body = realm_body(slug, &self.smtp, self.login_theme.as_deref());
        self.admin_post_idempotent(&format!("{}/admin/realms", self.base), &body)
            .await
    }

    async fn ensure_client(&self, slug: &str, redirect_uri: &str) -> Result<(), KcError> {
        // Public + PKCE only: quark never holds a client secret for the
        // tenant-facing login client.
        let body = json!({
            "clientId": "quark",
            "enabled": true,
            "protocol": "openid-connect",
            "publicClient": true,
            "standardFlowEnabled": true,
            "directAccessGrantsEnabled": false,
            "redirectUris": [redirect_uri],
            "attributes": { "pkce.code.challenge.method": "S256" },
        });
        self.admin_post_idempotent(&format!("{}/admin/realms/{slug}/clients", self.base), &body)
            .await
    }

    async fn ensure_groups_and_mapper(&self, slug: &str) -> Result<(), KcError> {
        for group in ["quark-admins", "quark-readers"] {
            self.admin_post_idempotent(
                &format!("{}/admin/realms/{slug}/groups", self.base),
                &json!({ "name": group }),
            )
            .await?;
        }

        let clients = self
            .admin_get(&format!(
                "{}/admin/realms/{slug}/clients?clientId=quark",
                self.base
            ))
            .await?;
        let client_uuid = clients
            .as_array()
            .and_then(|a| a.first())
            .and_then(|c| c.get("id"))
            .and_then(|v| v.as_str())
            .ok_or_else(|| {
                KcError("quark client not found; ensure_client must run first".to_string())
            })?;

        let mapper_body = json!({
            "name": "groups",
            "protocol": "openid-connect",
            "protocolMapper": "oidc-group-membership-mapper",
            "consentRequired": false,
            "config": {
                "claim.name": "groups",
                "full.path": "false",
                "id.token.claim": "true",
                "access.token.claim": "true",
                "userinfo.token.claim": "true",
            },
        });
        self.admin_post_idempotent(
            &format!(
                "{}/admin/realms/{slug}/clients/{client_uuid}/protocol-mappers/models",
                self.base
            ),
            &mapper_body,
        )
        .await
    }

    async fn ensure_user(&self, slug: &str, email: &str, group: &str) -> Result<String, KcError> {
        let query = url::form_urlencoded::Serializer::new(String::new())
            .append_pair("email", email)
            .append_pair("exact", "true")
            .finish();
        let lookup_url = format!("{}/admin/realms/{slug}/users?{query}", self.base);

        let existing = self.admin_get(&lookup_url).await?;
        if let Some(id) = first_user_id(&existing) {
            return Ok(id);
        }

        let token = self.admin_token().await?;
        let create_body = json!({
            "username": email,
            "email": email,
            "enabled": true,
            "emailVerified": false,
            "groups": [format!("/{group}")],
        });
        let create_url = format!("{}/admin/realms/{slug}/users", self.base);
        let mut resp = self.post_json(&create_url, &token, &create_body).await?;
        if matches!(
            resp.status(),
            StatusCode::UNAUTHORIZED | StatusCode::FORBIDDEN
        ) {
            // Mirrors `admin_post_idempotent`/`admin_get`: the cached token
            // may have expired (401) or lack the realm's freshly-added roles
            // (403) between `admin_token` and this POST, so fetch a fresh one
            // and retry exactly once before giving up.
            let token = self.fetch_token().await?;
            resp = self.post_json(&create_url, &token, &create_body).await?;
        }
        if resp.status() == StatusCode::CONFLICT {
            // Raced with another provisioning attempt for the same tenant;
            // the user now exists, look it up rather than erroring.
            let found = self.admin_get(&lookup_url).await?;
            return first_user_id(&found)
                .ok_or_else(|| KcError("user create raced but lookup found nothing".to_string()));
        }
        if !resp.status().is_success() {
            return Err(KcError(format!(
                "keycloak user create failed: {}",
                resp.status()
            )));
        }
        resp.headers()
            .get("location")
            .and_then(|v| v.to_str().ok())
            .and_then(|l| l.rsplit('/').next())
            .map(String::from)
            .ok_or_else(|| KcError("user create response had no Location header".to_string()))
    }

    async fn send_set_password_email(&self, slug: &str, user_id: &str) -> Result<(), KcError> {
        let url = format!(
            "{}/admin/realms/{slug}/users/{user_id}/execute-actions-email",
            self.base
        );
        self.admin_post_idempotent(&url, &json!(["UPDATE_PASSWORD"]))
            .await
    }
}

/// Extracts `id` from the first element of a Keycloak list-users response.
fn first_user_id(v: &serde_json::Value) -> Option<String> {
    v.as_array()?.first()?.get("id")?.as_str().map(String::from)
}

/// Builds the realm-create request body for `ensure_realm`. `login_theme` is
/// only included as `loginTheme` when `Some`: leaving it out entirely (not
/// even `null`) keeps Keycloak's own default login theme in place, since a
/// `loginTheme` naming a theme that isn't deployed on the server breaks that
/// realm's login page.
fn realm_body(slug: &str, smtp: &SmtpConfig, login_theme: Option<&str>) -> serde_json::Value {
    let mut body = json!({
        "realm": slug,
        "enabled": true,
        "sslRequired": "external",
        "registrationAllowed": false,
        "loginWithEmailAllowed": true,
        "duplicateEmailsAllowed": false,
        "smtpServer": smtp.to_json(),
    });
    if let Some(theme) = login_theme {
        body["loginTheme"] = json!(theme);
    }
    body
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn realm_body_omits_login_theme_when_none() {
        let body = realm_body("acme", &SmtpConfig::default(), None);
        assert_eq!(body["realm"], "acme");
        assert!(body.get("loginTheme").is_none());
    }

    #[test]
    fn realm_body_includes_login_theme_when_set() {
        let body = realm_body("acme", &SmtpConfig::default(), Some("quark-branded"));
        assert_eq!(body["loginTheme"], "quark-branded");
    }
}
