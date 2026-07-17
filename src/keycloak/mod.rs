//! Keycloak-hosted auth (multi-tenancy P2e): a mockable admin trait for the
//! per-tenant realm provisioning that P2e needs (Task 2 wires the actual
//! provisioning flow; this module only lays the foundation). Opt-in via
//! `QUARK_KEYCLOAK_BASE_URL`; unset, the whole feature is off and nothing here
//! runs.
//!
//! quark never handles a tenant user's password: the `quark` client Keycloak
//! provisions is public + PKCE (see `client::HttpKeycloakAdmin::ensure_client`),
//! and no client secret for it is ever stored.

pub mod client;

use async_trait::async_trait;

/// An error from a Keycloak admin call. Wraps a plain message; the HTTP
/// implementation is thin (see `client.rs`) so there is no richer variant set
/// to map onto yet.
#[derive(Debug, Clone, PartialEq)]
pub struct KcError(pub String);

impl std::fmt::Display for KcError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl std::error::Error for KcError {}

impl From<String> for KcError {
    fn from(s: String) -> Self {
        KcError(s)
    }
}

/// The per-tenant realm provisioning operations P2e needs, behind a trait so
/// the calling logic (Task 2) is testable with a mock and no live Keycloak
/// server. Every method is idempotent: safe to call again on a partially
/// provisioned tenant (e.g. after a crash mid-provision).
#[async_trait]
pub trait KeycloakAdmin: Send + Sync {
    /// Creates the tenant's realm (named `slug`) with the SMTP server
    /// configured. A `409 Conflict` (realm already exists) is treated as
    /// success.
    async fn ensure_realm(&self, slug: &str) -> Result<(), KcError>;

    /// Creates the public, PKCE-only `quark` client in the tenant's realm,
    /// pointed at `redirect_uri`. Idempotent (409 = ok).
    async fn ensure_client(&self, slug: &str, redirect_uri: &str) -> Result<(), KcError>;

    /// Creates the `quark-admins`/`quark-readers` groups and the `groups`
    /// claim mapper on the `quark` client. Idempotent (409 = ok); requires
    /// `ensure_client` to have run first (the mapper attaches to the client).
    async fn ensure_groups_and_mapper(&self, slug: &str) -> Result<(), KcError>;

    /// Creates the user (or returns the existing one's id) in `group`.
    /// Idempotent: a user that already exists by email is looked up, not
    /// duplicated.
    async fn ensure_user(&self, slug: &str, email: &str, group: &str) -> Result<String, KcError>;

    /// Triggers Keycloak's `UPDATE_PASSWORD` required-action email so the user
    /// sets their own password. quark never sees or stores it.
    async fn send_set_password_email(&self, slug: &str, user_id: &str) -> Result<(), KcError>;
}

/// SMTP settings for the realms Keycloak provisions, read once from the
/// environment. Every field defaults to empty/off; Keycloak realms function
/// without an SMTP server configured (only outbound email, e.g. the
/// set-password action, would silently fail to send).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct SmtpConfig {
    pub host: String,
    pub port: String,
    pub user: String,
    pub password: String,
    pub from: String,
    pub starttls: bool,
}

impl SmtpConfig {
    pub fn from_env() -> SmtpConfig {
        SmtpConfig {
            host: std::env::var("QUARK_KEYCLOAK_SMTP_HOST").unwrap_or_default(),
            port: std::env::var("QUARK_KEYCLOAK_SMTP_PORT").unwrap_or_default(),
            user: std::env::var("QUARK_KEYCLOAK_SMTP_USER").unwrap_or_default(),
            password: std::env::var("QUARK_KEYCLOAK_SMTP_PASSWORD").unwrap_or_default(),
            from: std::env::var("QUARK_KEYCLOAK_SMTP_FROM").unwrap_or_default(),
            starttls: matches!(
                std::env::var("QUARK_KEYCLOAK_SMTP_STARTTLS").as_deref(),
                Ok("true") | Ok("1")
            ),
        }
    }

    /// The realm `smtpServer` JSON block Keycloak's realm-create API expects.
    pub fn to_json(&self) -> serde_json::Value {
        serde_json::json!({
            "host": self.host,
            "port": self.port,
            "user": self.user,
            "password": self.password,
            "from": self.from,
            "starttls": self.starttls.to_string(),
            "auth": (!self.user.is_empty()).to_string(),
        })
    }
}

/// Opt-in configuration for the Keycloak connector. `from_env` returns `None`
/// unless `QUARK_KEYCLOAK_BASE_URL` is set and non-empty, which keeps the
/// whole feature off by default.
#[derive(Clone, Debug, PartialEq)]
pub struct KeycloakConfig {
    /// Trailing-slash-trimmed base URL of the Keycloak server, e.g.
    /// `https://kc.example.com`.
    pub base_url: String,
    pub admin_client_id: String,
    pub admin_client_secret: String,
    pub smtp: SmtpConfig,
}

impl KeycloakConfig {
    pub fn from_env() -> Option<KeycloakConfig> {
        Self::from_parts(
            &std::env::var("QUARK_KEYCLOAK_BASE_URL").unwrap_or_default(),
            &std::env::var("QUARK_KEYCLOAK_ADMIN_CLIENT_ID").unwrap_or_default(),
            &std::env::var("QUARK_KEYCLOAK_ADMIN_CLIENT_SECRET").unwrap_or_default(),
            SmtpConfig::from_env(),
        )
    }

    /// Builds a config from explicit parts (used by `from_env` and tests, so
    /// tests do not need to mutate process env — mirrors
    /// `sheets::SheetsConfig::from_parts`).
    pub fn from_parts(
        base_url: &str,
        admin_client_id: &str,
        admin_client_secret: &str,
        smtp: SmtpConfig,
    ) -> Option<KeycloakConfig> {
        if base_url.is_empty() {
            return None;
        }
        Some(KeycloakConfig {
            base_url: base_url.trim_end_matches('/').to_string(),
            admin_client_id: admin_client_id.to_string(),
            admin_client_secret: admin_client_secret.to_string(),
            smtp,
        })
    }
}

/// The issuer URL for a tenant's realm: `{base}/realms/{slug}`, with any
/// trailing slash on `base` trimmed first so the result never has a doubled
/// slash.
pub fn derive_issuer(base: &str, slug: &str) -> String {
    format!("{}/realms/{slug}", base.trim_end_matches('/'))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    /// Records every call (as a formatted string) so tests can assert call
    /// shape and order without a live Keycloak server.
    #[derive(Default)]
    struct MockKeycloakAdmin {
        calls: Mutex<Vec<String>>,
        next_user_id: Mutex<Option<String>>,
    }

    #[async_trait]
    impl KeycloakAdmin for MockKeycloakAdmin {
        async fn ensure_realm(&self, slug: &str) -> Result<(), KcError> {
            self.calls
                .lock()
                .unwrap()
                .push(format!("ensure_realm({slug})"));
            Ok(())
        }

        async fn ensure_client(&self, slug: &str, redirect_uri: &str) -> Result<(), KcError> {
            self.calls
                .lock()
                .unwrap()
                .push(format!("ensure_client({slug},{redirect_uri})"));
            Ok(())
        }

        async fn ensure_groups_and_mapper(&self, slug: &str) -> Result<(), KcError> {
            self.calls
                .lock()
                .unwrap()
                .push(format!("ensure_groups_and_mapper({slug})"));
            Ok(())
        }

        async fn ensure_user(
            &self,
            slug: &str,
            email: &str,
            group: &str,
        ) -> Result<String, KcError> {
            self.calls
                .lock()
                .unwrap()
                .push(format!("ensure_user({slug},{email},{group})"));
            Ok(self
                .next_user_id
                .lock()
                .unwrap()
                .clone()
                .unwrap_or_else(|| "user-1".to_string()))
        }

        async fn send_set_password_email(&self, slug: &str, user_id: &str) -> Result<(), KcError> {
            self.calls
                .lock()
                .unwrap()
                .push(format!("send_set_password_email({slug},{user_id})"));
            Ok(())
        }
    }

    #[tokio::test]
    async fn mock_records_calls_through_the_trait_object() {
        let mock = MockKeycloakAdmin::default();
        *mock.next_user_id.lock().unwrap() = Some("kc-user-42".to_string());
        let admin: &dyn KeycloakAdmin = &mock;

        admin.ensure_realm("acme").await.unwrap();
        admin
            .ensure_client("acme", "https://acme.quarkus.example/admin/callback")
            .await
            .unwrap();
        admin.ensure_groups_and_mapper("acme").await.unwrap();
        let uid = admin
            .ensure_user("acme", "owner@acme.example", "quark-admins")
            .await
            .unwrap();
        assert_eq!(uid, "kc-user-42");
        admin.send_set_password_email("acme", &uid).await.unwrap();

        assert_eq!(
            *mock.calls.lock().unwrap(),
            vec![
                "ensure_realm(acme)".to_string(),
                "ensure_client(acme,https://acme.quarkus.example/admin/callback)".to_string(),
                "ensure_groups_and_mapper(acme)".to_string(),
                "ensure_user(acme,owner@acme.example,quark-admins)".to_string(),
                "send_set_password_email(acme,kc-user-42)".to_string(),
            ]
        );
    }

    #[test]
    fn derive_issuer_appends_realms_path() {
        assert_eq!(
            derive_issuer("https://kc.example.com", "acme"),
            "https://kc.example.com/realms/acme"
        );
    }

    #[test]
    fn derive_issuer_trims_one_trailing_slash_on_base() {
        assert_eq!(
            derive_issuer("https://kc.example.com/", "acme"),
            "https://kc.example.com/realms/acme"
        );
    }

    #[test]
    fn keycloak_config_from_parts_is_none_without_base_url() {
        assert!(KeycloakConfig::from_parts("", "id", "secret", SmtpConfig::default()).is_none());
    }

    #[test]
    fn keycloak_config_from_parts_trims_trailing_slash() {
        let cfg = KeycloakConfig::from_parts(
            "https://kc.example.com/",
            "admin-cli",
            "s3cr3t",
            SmtpConfig::default(),
        )
        .unwrap();
        assert_eq!(cfg.base_url, "https://kc.example.com");
        assert_eq!(cfg.admin_client_id, "admin-cli");
        assert_eq!(cfg.admin_client_secret, "s3cr3t");
    }

    #[test]
    fn smtp_config_to_json_reports_auth_true_only_with_a_user() {
        let none = SmtpConfig::default();
        assert_eq!(none.to_json()["auth"], "false");

        let with_user = SmtpConfig {
            host: "smtp.example.com".into(),
            port: "587".into(),
            user: "bot@example.com".into(),
            password: "pw".into(),
            from: "noreply@example.com".into(),
            starttls: true,
        };
        let v = with_user.to_json();
        assert_eq!(v["host"], "smtp.example.com");
        assert_eq!(v["auth"], "true");
        assert_eq!(v["starttls"], "true");
    }
}
