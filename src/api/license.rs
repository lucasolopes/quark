//! Which edition this process is running (LUC-146).
//!
//! The strong guarantee is the build: a Community binary contains none of
//! `src/ee/`, so there is nothing to unlock. This module is the second layer,
//! for the day a paying self-hoster runs the Enterprise build and the binary
//! has to know whether that installation is covered.
//!
//! Only the seam is here. There is no license server, no telemetry, and no
//! network call: `resolve` reads the environment once at boot and nothing else.
//! Real validation (an Ed25519-signed token verified offline against a public
//! key baked into the binary) is deliberately not built yet. Building the
//! licensing machine before the first paying customer is work thrown away.
//!
//! Degradation rule, decided in
//! `docs/specs/2026-08-03-luc19-open-core-design.md` (D5): an expired or
//! missing license never takes the service down and never deletes data. It
//! blocks creating things through Enterprise features and leaves the rest
//! readable. A shortener that stops redirecting because a license lapsed is an
//! incident, not a business model.

/// The edition and, when Enterprise, what the license says about it.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub enum LicenseStatus {
    /// AGPL core only. Always this in a build without `--features ee`.
    #[default]
    Community,
    /// Enterprise build with a license present.
    Enterprise {
        /// Unix seconds the license stops covering new Enterprise writes.
        /// `None` means no expiry was stated.
        expires_at: Option<u64>,
        /// Seat allowance, when the license states one.
        seats: Option<u32>,
    },
    /// Enterprise build with no license, or one that no longer covers this
    /// installation. Reads keep working; Enterprise creation is refused.
    Unlicensed,
}

impl LicenseStatus {
    /// Resolves the status once, at boot.
    ///
    /// Community builds short-circuit to [`LicenseStatus::Community`] without
    /// reading anything. Enterprise builds look at `QUARK_LICENSE_KEY`; until
    /// signature verification lands, any non-empty value is taken at face
    /// value, which is enough to exercise the seam end to end and is not
    /// relied on for enforcement.
    pub fn resolve() -> LicenseStatus {
        #[cfg(not(feature = "ee"))]
        {
            LicenseStatus::Community
        }
        #[cfg(feature = "ee")]
        {
            match std::env::var("QUARK_LICENSE_KEY") {
                Ok(v) if !v.trim().is_empty() => LicenseStatus::Enterprise {
                    expires_at: None,
                    seats: None,
                },
                _ => LicenseStatus::Unlicensed,
            }
        }
    }

    /// Whether Enterprise features may create or modify things right now.
    ///
    /// The single checkpoint. Enterprise handlers consult this instead of
    /// inspecting the enum, so tightening the rule later is one edit here and
    /// not a sweep across handlers.
    pub fn allows_enterprise_writes(&self) -> bool {
        matches!(self, LicenseStatus::Enterprise { .. })
    }

    /// Short name for `/admin/me`, so the panel can label the edition.
    pub fn edition(&self) -> &'static str {
        match self {
            LicenseStatus::Community => "community",
            LicenseStatus::Enterprise { .. } => "enterprise",
            LicenseStatus::Unlicensed => "unlicensed",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn community_never_allows_enterprise_writes() {
        assert!(!LicenseStatus::Community.allows_enterprise_writes());
        assert!(!LicenseStatus::Unlicensed.allows_enterprise_writes());
        assert!(LicenseStatus::Enterprise {
            expires_at: None,
            seats: None,
        }
        .allows_enterprise_writes());
    }

    #[test]
    fn edition_names_are_stable() {
        // `/admin/me` ships these to the panel; renaming one is an API change.
        assert_eq!(LicenseStatus::Community.edition(), "community");
        assert_eq!(LicenseStatus::Unlicensed.edition(), "unlicensed");
        assert_eq!(
            LicenseStatus::Enterprise {
                expires_at: None,
                seats: None,
            }
            .edition(),
            "enterprise"
        );
    }

    #[test]
    fn default_is_community() {
        assert_eq!(LicenseStatus::default(), LicenseStatus::Community);
    }

    /// A Community build has nothing to unlock, so `resolve` must ignore the
    /// environment entirely.
    #[cfg(not(feature = "ee"))]
    #[test]
    fn community_build_ignores_the_license_env() {
        std::env::set_var("QUARK_LICENSE_KEY", "anything");
        assert_eq!(LicenseStatus::resolve(), LicenseStatus::Community);
        std::env::remove_var("QUARK_LICENSE_KEY");
    }
}
