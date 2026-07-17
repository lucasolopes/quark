//! DNS seam for custom-domain verification (multi-tenancy P3). The only
//! caller is the `/admin/domains/:id/verify` endpoint, checking whether the
//! tenant published the expected `_quark-verify.<host>` TXT record. Never
//! call this from the redirect hot path — a DNS round trip has no place
//! there.
use async_trait::async_trait;
use hickory_resolver::config::{ResolverConfig, CLOUDFLARE};
use hickory_resolver::net::runtime::TokioRuntimeProvider;
use hickory_resolver::proto::rr::RData;
use hickory_resolver::proto::rr::RecordType;
use hickory_resolver::Resolver;
use std::time::Duration;

/// A TXT lookup failed or ran past its time budget.
#[derive(Debug)]
pub enum DnsError {
    Timeout,
    Backend(String),
}

impl std::fmt::Display for DnsError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            DnsError::Timeout => write!(f, "dns lookup timed out"),
            DnsError::Backend(e) => write!(f, "dns lookup failed: {e}"),
        }
    }
}

impl std::error::Error for DnsError {}

/// Time budget for a single TXT lookup. Long enough for a real resolver
/// round trip, short enough that a slow or unresponsive name server can't
/// hang the admin request that called it.
const LOOKUP_TIMEOUT: Duration = Duration::from_secs(5);

/// Seam over TXT record lookup, so tests can inject known records instead of
/// hitting a real name server.
#[async_trait]
pub trait Dns: Send + Sync {
    async fn lookup_txt(&self, name: &str) -> Result<Vec<String>, DnsError>;
}

/// Real resolver, backed by `hickory-resolver` against Cloudflare's `1.1.1.1`
/// (fixed upstream rather than the host's `/etc/resolv.conf`, so behavior is
/// the same across containers that may not have one configured).
pub struct HickoryDns {
    resolver: Resolver<TokioRuntimeProvider>,
}

impl HickoryDns {
    pub fn new() -> Result<Self, DnsError> {
        let config = ResolverConfig::udp_and_tcp(&CLOUDFLARE);
        let resolver = Resolver::builder_with_config(config, TokioRuntimeProvider::default())
            .build()
            .map_err(|e| DnsError::Backend(e.to_string()))?;
        Ok(Self { resolver })
    }
}

#[async_trait]
impl Dns for HickoryDns {
    async fn lookup_txt(&self, name: &str) -> Result<Vec<String>, DnsError> {
        let lookup =
            tokio::time::timeout(LOOKUP_TIMEOUT, self.resolver.lookup(name, RecordType::TXT))
                .await
                .map_err(|_| DnsError::Timeout)?
                .map_err(|e| DnsError::Backend(e.to_string()))?;
        let mut out = Vec::new();
        for record in lookup.answers() {
            if let RData::TXT(txt) = &record.data {
                let joined: String = txt
                    .txt_data
                    .iter()
                    .map(|chunk| String::from_utf8_lossy(chunk).into_owned())
                    .collect();
                out.push(joined);
            }
        }
        Ok(out)
    }
}

/// No-op DNS: always returns zero TXT records. Used wherever a real resolver
/// isn't wired up (tests that don't exercise `verify`, and any deploy that
/// never calls it).
pub struct NullDns;

#[async_trait]
impl Dns for NullDns {
    async fn lookup_txt(&self, _name: &str) -> Result<Vec<String>, DnsError> {
        Ok(Vec::new())
    }
}
