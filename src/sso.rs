//! SSO email-domain discovery (LUC-57, Home Realm Discovery): a tenant with its
//! own OIDC can register verified email domains (e.g. `acme.com`) so a user who
//! types their email on the central login is routed straight to that tenant's
//! SSO without knowing the org slug. Domains are verified by DNS TXT (reusing
//! the P3 `Dns` seam) and are unique across tenants, so only one tenant can own
//! a given email domain. Cloud-only.
use crate::domain::DomainStatus;
use crate::tenant::TenantId;
use serde::{Deserialize, Serialize};

/// A tenant's verified (or pending) email domain for SSO discovery. Mirrors the
/// P3 `Domain` shape (id/token/status/verified_at), but keyed on an email
/// `domain` rather than a redirect `host`, and kept in a separate table off the
/// redirect hot path.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SsoEmailDomain {
    pub id: u64,
    pub tenant_id: TenantId,
    pub domain: String,
    pub token: String,
    pub status: DomainStatus,
    pub created: u64,
    pub verified_at: Option<u64>,
}

/// Extracts the domain part of an email address, lowercased, for SSO discovery.
///
/// Uses the substring after the LAST `@`. Returns `None` for anything that
/// isn't a usable domain: no `@`, an empty local part (`@x.com`), an empty
/// domain (`a@`), a domain with no dot (`a@nodot`), or a domain with a leading
/// or trailing dot or embedded whitespace. A `None` simply means "no SSO
/// routing" — it is never an error, so the discovery endpoint stays uniform.
pub fn normalize_email_domain(email: &str) -> Option<String> {
    let at = email.rfind('@')?;
    if at == 0 {
        return None; // empty local part
    }
    let domain = email[at + 1..].trim().to_ascii_lowercase();
    if domain.is_empty()
        || !domain.contains('.')
        || domain.starts_with('.')
        || domain.ends_with('.')
        || domain.chars().any(|c| c.is_whitespace())
    {
        return None;
    }
    Some(domain)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalizes_and_lowercases() {
        assert_eq!(
            normalize_email_domain("a@ACME.com").as_deref(),
            Some("acme.com")
        );
        assert_eq!(
            normalize_email_domain("x@sub.Acme.CO").as_deref(),
            Some("sub.acme.co")
        );
    }

    #[test]
    fn takes_the_part_after_the_last_at() {
        assert_eq!(
            normalize_email_domain("weird@name@acme.com").as_deref(),
            Some("acme.com")
        );
    }

    #[test]
    fn rejects_malformed() {
        assert_eq!(normalize_email_domain("bad"), None);
        assert_eq!(normalize_email_domain("a@"), None);
        assert_eq!(normalize_email_domain("a@nodot"), None);
        assert_eq!(normalize_email_domain("@x.com"), None); // empty local part
        assert_eq!(normalize_email_domain("a@.acme.com"), None); // leading dot
        assert_eq!(normalize_email_domain("a@acme.com."), None); // trailing dot
        assert_eq!(normalize_email_domain("a@ac me.com"), None); // whitespace
    }
}
