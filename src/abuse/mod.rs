pub mod blocklist;
pub mod ratelimit;

use std::collections::HashSet;
use std::net::IpAddr;

/// Host da URL em minúsculas, sem porta. `None` se não parsear ou não tiver host.
pub fn extract_host(url: &str) -> Option<String> {
    let u = url::Url::parse(url).ok()?;
    u.host_str().map(|h| h.to_ascii_lowercase())
}

/// `true` para destinos de rede interna que um encurtador público não deve encurtar:
/// `localhost`/`*.localhost`, ou IP literal loopback/privado/link-local/unspecified.
/// NÃO resolve DNS — só decide sobre IP literal e nomes óbvios.
pub fn is_internal_host(host: &str) -> bool {
    let h = host.to_ascii_lowercase();
    if h == "localhost" || h.ends_with(".localhost") {
        return true;
    }
    // IPv6 literal em URL vem entre colchetes; url crate já os remove no host_str,
    // mas normalizamos por segurança.
    let h_ip = h.trim_start_matches('[').trim_end_matches(']');
    match h_ip.parse::<IpAddr>() {
        Ok(IpAddr::V4(v4)) => {
            v4.is_private()
                || v4.is_loopback()
                || v4.is_link_local()
                || v4.is_unspecified()
                || v4.is_broadcast()
        }
        Ok(IpAddr::V6(v6)) => v6.is_loopback() || v6.is_unspecified(),
        Err(_) => false, // nome não-IP e não-localhost: não é "interno" por si só
    }
}

/// `true` se `host` ou qualquer domínio-pai está no conjunto (case-insensitive).
/// Ex.: set={evil.com} bloqueia evil.com, x.evil.com, a.b.evil.com.
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
    fn extract_host_normaliza_e_tira_porta() {
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
        // "http:///semhost" NÃO cai aqui: pela WHATWG URL spec (special authority
        // ignore slashes), a `url` crate 2.5.8 trata a barra extra como parte do
        // marcador de authority e lê "semhost" como host (path fica "/"). Um caso
        // real de URL válida sem host é um scheme sem authority, ex. `file:///`.
        assert_eq!(extract_host("file:///semhost"), None);
    }

    #[test]
    fn is_internal_host_pega_loopback_privado_localhost() {
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
            assert!(is_internal_host(h), "deveria bloquear {h}");
        }
    }

    #[test]
    fn is_internal_host_libera_publicos() {
        for h in ["example.com", "8.8.8.8", "1.1.1.1", "meusite.com.br"] {
            assert!(!is_internal_host(h), "não deveria bloquear {h}");
        }
    }

    #[test]
    fn host_in_blocklist_casa_dominio_e_subdominio() {
        let mut set = HashSet::new();
        set.insert("evil.com".to_string());
        assert!(host_in_blocklist("evil.com", &set));
        assert!(host_in_blocklist("x.evil.com", &set));
        assert!(host_in_blocklist("a.b.evil.com", &set));
        assert!(host_in_blocklist("EVIL.COM", &set)); // case-insensitive
        assert!(!host_in_blocklist("eviltwin.com", &set));
        assert!(!host_in_blocklist("evil.com.br", &set));
    }
}
