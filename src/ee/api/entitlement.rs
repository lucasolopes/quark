//! Enterprise plan enforcement (LUC-41). Covered by `src/ee/LICENSE`, not by
//! the AGPL.
//!
//! Placeholder for this task only: the real plan catalog and quota checks
//! land in the next task (LUC-41 phase 1, task 4). Until then these resolve
//! every check to `Ok`, same as the Community seam in `src/api/entitlement.rs`,
//! so `--features ee` builds and the reexport in that module has something to
//! point at.

use crate::api::entitlement::{Denied, Feature, Quota};
use crate::api::AppState;
use crate::tenant::TenantId;

/// Placeholder: always allowed. Replaced by the real plan check in the next
/// task.
pub async fn require(_st: &AppState, _tenant: TenantId, _f: Feature) -> Result<(), Denied> {
    Ok(())
}

/// Placeholder: no ceiling applies. Replaced by the real quota check in the
/// next task.
pub async fn require_quota(
    _st: &AppState,
    _tenant: TenantId,
    _q: Quota,
    _current: u64,
) -> Result<(), Denied> {
    Ok(())
}
