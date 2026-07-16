//! Tenancy domain model. A Tenant owns all data; a User is a global identity;
//! a Membership links a User to a Tenant with a Role. In OSS mode exactly one
//! tenant exists (`DEFAULT_TENANT`); cloud mode has many.
use serde::{Deserialize, Serialize};
use crate::auth::Scope;

/// Opaque tenant identifier. `0` is the default/OSS tenant.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct TenantId(pub u64);

/// The single implicit tenant in OSS mode, and the tenant existing data is
/// migrated into.
pub const DEFAULT_TENANT: TenantId = TenantId(0);

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Tenant {
    pub id: TenantId,
    pub name: String,
    pub slug: String,
    pub created: u64,
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
}
