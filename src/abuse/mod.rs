pub mod blocklist;
pub mod ratelimit;

use std::collections::HashSet;
use std::net::IpAddr;

/// Lowercased URL host, without port. `None` if it doesn't parse or has no host.
pub fn extract_host(url: &str) -> Option<String> {
    let u = url::Url::parse(url).ok()?;
    u.host_str().map(|h| h.to_ascii_lowercase())
}

/// `true` for internal network destinations that a public shortener should not shorten:
/// `localhost`/`*.localhost`, or a literal loopback/private/link-local/unspecified IP.
/// Does NOT resolve DNS — it only decides on literal IPs and obvious names.
pub fn is_internal_host(host: &str) -> bool {
    let h = host.to_ascii_lowercase();
    if h == "localhost" || h.ends_with(".localhost") {
        return true;
    }
    let h_ip = h.trim_start_matches('[').trim_end_matches(']');
    match h_ip.parse::<IpAddr>() {
        Ok(IpAddr::V4(v4)) => {
            v4.is_private()
                || v4.is_loopback()
                || v4.is_link_local()
                || v4.is_unspecified()
                || v4.is_broadcast()
        }
        Ok(IpAddr::V6(v6)) => {
            if let Some(v4) = v6.to_ipv4_mapped() {
                return v4.is_private()
                    || v4.is_loopback()
                    || v4.is_link_local()
                    || v4.is_unspecified()
                    || v4.is_broadcast();
            }
            let seg = v6.segments();
            let ula = (seg[0] & 0xfe00) == 0xfc00;
            let link_local = (seg[0] & 0xffc0) == 0xfe80;
            v6.is_loopback() || v6.is_unspecified() || ula || link_local
        }
        Err(_) => false,
    }
}

/// `true` if `host` or any parent domain is in the set (case-insensitive).
/// E.g.: set={evil.com} blocks evil.com, x.evil.com, a.b.evil.com.
pub fn host_in_blocklist(host: &str, set: &HashSet<String>) -> bool {
    let h = host.to_ascii_lowercase();
    let mut rest = h.as_str();
    loop {
        if set.contains(rest) {
            return true;
        }
        match rest.find('.') {
            Some(i) => rest = &rest[i + 1..],
            None => return false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{extract_host, host_in_blocklist, is_internal_host};
    use std::collections::HashSet;

    #[test]
    fn extract_host_normalizes_and_strips_port() {
        assert_eq!(
            extract_host("https://Example.COM/a/b?x=1"),
            Some("example.com".into())
        );
        assert_eq!(extract_host("http://host:8080/x"), Some("host".into()));
        assert_eq!(
            extract_host("http://127.0.0.1:3000"),
            Some("127.0.0.1".into())
        );
        assert_eq!(extract_host("not a url"), None);
        assert_eq!(extract_host("file:///semhost"), None);
    }

    #[test]
    fn is_internal_host_catches_loopback_private_localhost() {
        for h in [
            "localhost",
            "foo.localhost",
            "127.0.0.1",
            "10.0.0.5",
            "192.168.1.1",
            "172.16.0.1",
            "169.254.1.1",
            "0.0.0.0",
            "::1",
        ] {
            assert!(is_internal_host(h), "should block {h}");
        }
    }

    #[test]
    fn is_internal_host_allows_public_hosts() {
        for h in ["example.com", "8.8.8.8", "1.1.1.1", "mysite.com.br"] {
            assert!(!is_internal_host(h), "should not block {h}");
        }
    }

    #[test]
    fn is_internal_host_catches_internal_and_mapped_ipv6() {
        for h in [
            "::1",
            "::",
            "[fc00::1]",
            "[fe80::1]",
            "[::ffff:127.0.0.1]",
            "[::ffff:10.0.0.1]",
        ] {
            assert!(is_internal_host(h), "should block {h}");
        }
    }

    #[test]
    fn is_internal_host_allows_public_ipv6() {
        assert!(!is_internal_host("[2606:4700::1111]"));
        assert!(!is_internal_host("[::ffff:8.8.8.8]"));
    }

    #[test]
    fn host_in_blocklist_matches_domain_and_subdomain() {
        let mut set = HashSet::new();
        set.insert("evil.com".to_string());
        assert!(host_in_blocklist("evil.com", &set));
        assert!(host_in_blocklist("x.evil.com", &set));
        assert!(host_in_blocklist("a.b.evil.com", &set));
        assert!(host_in_blocklist("EVIL.COM", &set));
        assert!(!host_in_blocklist("eviltwin.com", &set));
        assert!(!host_in_blocklist("evil.com.br", &set));
    }
}
