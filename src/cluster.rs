/// Startup guardrail for multi-node deployments.
///
/// When `QUARK_STRICT_CLUSTER` is set the operator is declaring a real cluster,
/// so both shared-state dependencies must be wired: `QUARK_DATABASE_URL` (the
/// shared store) and `QUARK_VALKEY_URL` (shared rate-limit plus cross-node
/// cache invalidation). Without them a "multi-node" deployment
/// silently degrades: per-node LMDB files that do not share links, N-times the
/// intended rate limit, and stale caches. This is a pure decision so it can be
/// unit-tested; `main` reads the env, calls it early, and exits non-zero on
/// `Err`. When strict is false the function always returns `Ok` and single-node
/// behavior is untouched.
pub fn cluster_preflight(strict: bool, has_pg: bool, has_valkey: bool) -> Result<(), ClusterError> {
    if !strict {
        return Ok(());
    }
    if has_pg && has_valkey {
        return Ok(());
    }
    let mut missing = Vec::new();
    if !has_pg {
        missing.push("QUARK_DATABASE_URL (shared store)");
    }
    if !has_valkey {
        missing.push("QUARK_VALKEY_URL (shared rate-limit + cross-node invalidation)");
    }
    Err(ClusterError::MissingSharedState(missing))
}

/// Why the cluster preflight refused to start.
///
/// A typed error rather than a `String` so the caller can tell this apart from
/// any other boot failure, and so the missing names stay structured data until
/// the moment they are rendered.
#[non_exhaustive]
#[derive(Debug, thiserror::Error)]
pub enum ClusterError {
    #[error(
        "QUARK_STRICT_CLUSTER is set but a real multi-node cluster needs the shared-state \
         dependencies, and these are missing: {}. Wire them, or unset QUARK_STRICT_CLUSTER \
         to run single-node.",
        .0.join(", ")
    )]
    MissingSharedState(Vec<&'static str>),
}

#[cfg(test)]
mod tests {
    use super::{cluster_preflight, ClusterError};

    /// The tests assert on the rendered message, which is what the operator
    /// sees at boot.
    fn format_err(e: ClusterError) -> String {
        e.to_string()
    }

    #[test]
    fn strict_with_both_present_is_ok() {
        assert!(cluster_preflight(true, true, true).is_ok());
    }

    #[test]
    fn strict_without_postgres_errors_and_names_it() {
        let err = format_err(cluster_preflight(true, false, true).unwrap_err());
        assert!(err.contains("QUARK_DATABASE_URL"));
        assert!(!err.contains("QUARK_VALKEY_URL"));
    }

    #[test]
    fn strict_without_valkey_errors_and_names_it() {
        let err = format_err(cluster_preflight(true, true, false).unwrap_err());
        assert!(err.contains("QUARK_VALKEY_URL"));
        assert!(!err.contains("QUARK_DATABASE_URL"));
    }

    #[test]
    fn strict_without_either_names_both() {
        let err = format_err(cluster_preflight(true, false, false).unwrap_err());
        assert!(err.contains("QUARK_DATABASE_URL"));
        assert!(err.contains("QUARK_VALKEY_URL"));
    }

    #[test]
    fn non_strict_is_always_ok() {
        assert!(cluster_preflight(false, false, false).is_ok());
        assert!(cluster_preflight(false, true, false).is_ok());
        assert!(cluster_preflight(false, false, true).is_ok());
        assert!(cluster_preflight(false, true, true).is_ok());
    }
}
