//! Tenancy domain model. A Tenant owns all data; a User is a global identity;
//! a Membership links a User to a Tenant with a Role. In OSS mode exactly one
//! tenant exists (`DEFAULT_TENANT`); cloud mode has many.
use crate::auth::Scope;
use serde::{Deserialize, Serialize};

/// Opaque tenant identifier. `0` is the default/OSS tenant.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Default, Serialize, Deserialize,
)]
pub struct TenantId(pub u64);

/// The single implicit tenant in OSS mode, and the tenant existing data is
/// migrated into.
pub const DEFAULT_TENANT: TenantId = TenantId(0);

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Tenant {
    pub id: TenantId,
    /// Human-friendly display name; freely mutable.
    pub name: String,
    /// URL/realm-safe identifier, validated at create (`is_valid_slug`).
    /// IMMUTABLE by contract (LUC-51): no endpoint changes a tenant's slug
    /// after creation. It is baked into the auto-provisioned subdomain
    /// (`<slug>.<suffix>`, materialized as a `domains` row), the Keycloak realm
    /// name, and the derived OIDC issuer — renaming it would orphan the
    /// subdomain `domains` row and break login. A future rename feature MUST
    /// migrate those together; do not add a bare slug-update path.
    pub slug: String,
    pub created: u64,
}

/// Keycloak/operational realm names a tenant slug must never collide with
/// (multi-tenancy P2e: the slug becomes the realm name — see `is_valid_slug`).
const RESERVED_SLUGS: &[&str] = &[
    "master",
    "admin",
    "account",
    "account-console",
    "broker",
    "realms",
    "security-admin-console",
];

/// Validates a tenant slug against the DNS-label / Keycloak-realm-safe
/// charset (`^[a-z0-9]([a-z0-9-]{0,61}[a-z0-9])?$`, 1-63 chars, lowercase
/// alnum plus internal hyphens, no leading/trailing hyphen) and rejects the
/// small set of reserved Keycloak/operational realm names.
///
/// This must run before a `Tenant` is created and before any Keycloak call:
/// the slug is later used verbatim as a Keycloak realm name, in Admin-API
/// URL paths (`/admin/realms/{slug}/...`), and in the derived OIDC issuer
/// (`{base}/realms/{slug}`), so a malformed or colliding slug corrupts all
/// three (final-review finding, multi-tenancy P2e).
pub fn is_valid_slug(slug: &str) -> bool {
    if slug.is_empty() || slug.len() > 63 {
        return false;
    }
    let bytes = slug.as_bytes();
    let is_label_char = |b: u8| b.is_ascii_lowercase() || b.is_ascii_digit();
    if !is_label_char(bytes[0]) || !is_label_char(bytes[bytes.len() - 1]) {
        return false;
    }
    if !bytes.iter().all(|&b| is_label_char(b) || b == b'-') {
        return false;
    }
    !RESERVED_SLUGS
        .iter()
        .any(|reserved| reserved.eq_ignore_ascii_case(slug))
}

/// A global user identity, keyed by the OIDC `subject` (immutable), never email.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct User {
    pub id: u64,
    pub subject: String,
    pub email: String,
    pub display: String,
    pub created: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Role {
    Owner,
    Admin,
    Member,
    Viewer,
}

/// Many-to-many join between a user and a tenant, carrying the role.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Membership {
    pub user_id: u64,
    pub tenant_id: TenantId,
    pub role: Role,
    pub created: u64,
}

/// Maps a role to the permission scopes it grants. Kept as a function (not a
/// stored set) so roles can be split/extended later without a schema change.
pub fn role_scopes(role: Role) -> &'static [Scope] {
    match role {
        // Owner/Admin are superusers within their tenant (tenant-management
        // authorization — deleting/transferring the tenant — is enforced at the
        // handler layer in P2, not via a scope).
        Role::Owner | Role::Admin => &[Scope::Full],
        Role::Member => &[Scope::LinksWrite, Scope::LinksRead, Scope::Analytics],
        Role::Viewer => &[Scope::LinksRead, Scope::Analytics],
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::auth::Scope;

    #[test]
    fn default_tenant_is_zero() {
        assert_eq!(DEFAULT_TENANT, TenantId(0));
    }

    #[test]
    fn owner_covers_full() {
        assert!(role_scopes(Role::Owner).contains(&Scope::Full));
    }

    #[test]
    fn member_can_write_and_read_but_not_full() {
        let s = role_scopes(Role::Member);
        assert!(s.contains(&Scope::LinksWrite));
        assert!(s.contains(&Scope::LinksRead));
        assert!(s.contains(&Scope::Analytics));
        assert!(!s.contains(&Scope::Full));
    }

    #[test]
    fn tenant_id_roundtrips_through_json() {
        let t = TenantId(42);
        let j = serde_json::to_string(&t).unwrap();
        assert_eq!(serde_json::from_str::<TenantId>(&j).unwrap(), t);
    }

    #[test]
    fn viewer_is_read_only() {
        let s = role_scopes(Role::Viewer);
        assert!(s.contains(&Scope::LinksRead));
        assert!(s.contains(&Scope::Analytics));
        assert!(!s.contains(&Scope::LinksWrite));
        assert!(!s.contains(&Scope::Full));
    }

    #[test]
    fn valid_slugs_accepted() {
        assert!(is_valid_slug("acme"));
        assert!(is_valid_slug("acme-corp"));
        assert!(is_valid_slug("a"));
        assert!(is_valid_slug("a1-b2"));
        assert!(is_valid_slug(&"a".repeat(63)));
    }

    #[test]
    fn malformed_slugs_rejected() {
        assert!(!is_valid_slug(""));
        assert!(!is_valid_slug("Bad_Slug!"));
        assert!(!is_valid_slug("a/b"));
        assert!(!is_valid_slug(" "));
        assert!(!is_valid_slug("-x"));
        assert!(!is_valid_slug("x-"));
        assert!(!is_valid_slug(&"a".repeat(64)));
    }

    #[test]
    fn reserved_slugs_rejected_case_insensitive() {
        assert!(!is_valid_slug("master"));
        assert!(!is_valid_slug("MASTER"));
        assert!(!is_valid_slug("admin"));
        assert!(!is_valid_slug("account"));
        assert!(!is_valid_slug("account-console"));
        assert!(!is_valid_slug("broker"));
        assert!(!is_valid_slug("realms"));
        assert!(!is_valid_slug("security-admin-console"));
    }
}
