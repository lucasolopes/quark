pub mod ratelimit;

use std::net::IpAddr;

/// Lowercased URL host, without port. `None` if it doesn't parse or has no host.
pub fn extract_host(url: &str) -> Option<String> {
    let u = url::Url::parse(url).ok()?;
    u.host_str().map(|h| h.to_ascii_lowercase())
}

/// Whether an IP address is in a range the server must never be pointed at by a
/// user-supplied destination.
///
/// This is the single classification point, shared by the string-only guard
/// (`is_internal_host`, which runs on the request path) and by
/// `health::safe_to_probe` (which resolves DNS before the server itself fetches
/// the URL). Keeping one function is load-bearing: the two used to diverge, and
/// an IPv4-compatible address such as `::127.0.0.1` was rejected by the health
/// checker while passing link creation.
///
/// `IpAddr::is_global` is deliberately not used: it is still unstable behind
/// `#![feature(ip)]` (rust-lang/rust#27709), so the ranges are spelled out over
/// the stable `Ipv4Addr` / `Ipv6Addr` accessors.
pub fn is_internal_ip(ip: &IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => {
            let o = v4.octets();
            v4.is_loopback()
                || v4.is_private()
                || v4.is_link_local()
                || v4.is_unspecified()
                || v4.is_broadcast()
                || v4.is_documentation()
                || v4.is_multicast()
                // "this network", 0.0.0.0/8
                || o[0] == 0
                // carrier-grade NAT, 100.64.0.0/10
                || (o[0] == 100 && (64..128).contains(&o[1]))
        }
        IpAddr::V6(v6) => {
            if v6.is_loopback() || v6.is_unspecified() || v6.is_multicast() {
                return true;
            }
            // An IPv4-mapped (`::ffff:a.b.c.d`) or deprecated IPv4-compatible
            // (`::a.b.c.d`) address routes to the embedded v4 target on a
            // dual-stack host, so classify it by that v4 address.
            let seg = v6.segments();
            if v6.to_ipv4_mapped().is_some() || seg[..6].iter().all(|&x| x == 0) {
                let v4 = std::net::Ipv4Addr::new(
                    (seg[6] >> 8) as u8,
                    (seg[6] & 0xff) as u8,
                    (seg[7] >> 8) as u8,
                    (seg[7] & 0xff) as u8,
                );
                return is_internal_ip(&IpAddr::V4(v4));
            }
            // unique-local fc00::/7 or link-local fe80::/10
            (seg[0] & 0xfe00) == 0xfc00 || (seg[0] & 0xffc0) == 0xfe80
        }
    }
}

/// `true` for internal network destinations that a public shortener should not shorten:
/// `localhost`/`*.localhost`, or a literal IP in a range `is_internal_ip` rejects.
///
/// Does NOT resolve DNS — it only decides on literal IPs and obvious names, and
/// that is deliberate: this runs on the request path. A public hostname pointing
/// at `169.254.169.254` passes here, so any code path where the *server* then
/// fetches the URL must additionally go through `health::safe_to_probe`.
pub fn is_internal_host(host: &str) -> bool {
    let h = host.to_ascii_lowercase();
    if h == "localhost" || h.ends_with(".localhost") {
        return true;
    }
    let h_ip = h.trim_start_matches('[').trim_end_matches(']');
    match h_ip.parse::<IpAddr>() {
        Ok(ip) => is_internal_ip(&ip),
        Err(_) => false,
    }
}

#[cfg(test)]
mod tests {
    use super::{extract_host, is_internal_host, is_internal_ip};
    use std::net::IpAddr;

    /// Table of the ranges `is_internal_ip` must reject. The IPv4-compatible
    /// entries (`::7f00:1`, `::a9fe:a9fe`) are the regression: before the two
    /// classifiers were unified they passed `is_internal_host`, so a link could
    /// be created pointing at loopback or at cloud metadata.
    #[test]
    fn is_internal_ip_rejects_every_non_public_range() {
        for s in [
            "127.0.0.1",
            "10.1.2.3",
            "192.168.0.1",
            "172.16.0.1",
            "169.254.169.254", // cloud metadata (link-local)
            "0.0.0.0",         // unspecified
            "0.1.2.3",         // 0.0.0.0/8 "this network"
            "255.255.255.255", // broadcast
            "192.0.2.1",       // documentation TEST-NET-1
            "198.51.100.1",    // documentation TEST-NET-2
            "203.0.113.1",     // documentation TEST-NET-3
            "224.0.0.1",       // multicast
            "100.64.0.1",      // carrier-grade NAT
            "100.127.255.255", // carrier-grade NAT, upper bound
            "::1",
            "::",
            "fc00::1",                // unique-local
            "fe80::1",                // link-local
            "ff02::1",                // multicast
            "::ffff:127.0.0.1",       // IPv4-mapped loopback
            "::ffff:169.254.169.254", // IPv4-mapped metadata
            "::7f00:1",               // IPv4-compatible 127.0.0.1 (deprecated)
            "::a9fe:a9fe",            // IPv4-compatible 169.254.169.254
        ] {
            let ip: IpAddr = s.parse().expect("test vector parses");
            assert!(is_internal_ip(&ip), "{s} must be internal");
        }
    }

    #[test]
    fn is_internal_ip_allows_public_addresses() {
        for s in [
            "8.8.8.8",
            "1.1.1.1",
            "100.63.255.255", // just below the CGNAT block
            "100.128.0.1",    // just above the CGNAT block
            "2606:4700:4700::1111",
            "::ffff:8.8.8.8",
        ] {
            let ip: IpAddr = s.parse().expect("test vector parses");
            assert!(!is_internal_ip(&ip), "{s} must be public");
        }
    }

    /// The bracketed forms matter because they are what `extract_host` yields for
    /// an IPv6 URL host.
    #[test]
    fn is_internal_host_handles_bracketed_ipv4_compatible_ipv6() {
        assert!(is_internal_host("[::7f00:1]"));
        assert!(is_internal_host("[::a9fe:a9fe]"));
        assert!(is_internal_host("[::ffff:169.254.169.254]"));
        assert!(!is_internal_host("[2606:4700:4700::1111]"));
    }

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
}
