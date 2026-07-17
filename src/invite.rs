//! Team invite model (multi-tenancy P2c, cloud-only): a pending invitation for
//! an email to join a tenant with a given role, identified by a hashed token
//! sent out-of-band (email link). `get_invite_by_hash` is the one public,
//! tenant-less lookup: the accept flow only has the raw token before it knows
//! which tenant the invite belongs to, so that path runs on the bare pool,
//! unscoped, mirroring `get_domain_by_host` and `get_api_token_by_hash`.
use crate::tenant::{Role, TenantId};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Invite {
    pub id: u64,
    pub tenant_id: TenantId,
    pub email: String,
    pub role: Role,
    pub token_hash: String,
    pub invited_by: u64,
    pub created: u64,
    pub expires: u64,
    pub accepted_at: Option<u64>,
    pub accepted_by: Option<u64>,
}
