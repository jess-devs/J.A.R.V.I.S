//! Guard anti-SSRF: antes de que una tool le pegue a una URL (búsqueda web,
//! `fetch_page`, o una receta HTTP de `create_tool`), resuelve el host y
//! rechaza si alguna IP resultante es loopback, privada o link-local. Se
//! chequean las IPs resueltas y no el string del host, así que un hostname
//! de aspecto público que apunta a una IP privada tampoco pasa.
//!
//! Lo que NO cubre: la ventana entre este chequeo y el fetch real. Acá se
//! resuelve el host y se aprueba, pero después `reqwest` vuelve a resolver
//! por su cuenta — con un TTL corto, un atacante puede contestar una IP
//! pública en la primera consulta y una privada en la segunda (DNS
//! rebinding). Cerrarlo de verdad pide fijar la conexión a la IP ya
//! validada, vía un resolver propio o `ClientBuilder::resolve`.

use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};

use crate::errors::ToolError;

/// Rechaza si `url` resuelve a una dirección de red privada/local. `Ok(())`
/// si el host es público (o si no se pudo determinar nada sospechoso).
pub async fn ensure_public_host(url: &str) -> Result<(), ToolError> {
    let parsed =
        url::Url::parse(url).map_err(|_| ToolError::InvalidArgs(format!("URL inválida: {url}")))?;
    let host = parsed
        .host_str()
        .ok_or_else(|| ToolError::InvalidArgs(format!("URL sin host: {url}")))?;
    let port = parsed.port_or_known_default().unwrap_or(80);

    let addrs = tokio::net::lookup_host((host, port))
        .await
        .map_err(|e| ToolError::Execution(format!("no se pudo resolver el host '{host}': {e}")))?;

    for addr in addrs {
        let ip = addr.ip();
        if is_forbidden(ip) {
            return Err(ToolError::Execution(format!(
                "el host '{host}' resuelve a una dirección de red privada/local ({ip}), \
                 bloqueado por seguridad (SSRF)"
            )));
        }
    }
    Ok(())
}

fn is_forbidden(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => is_forbidden_v4(v4),
        IpAddr::V6(v6) => is_forbidden_v6(v6),
    }
}

fn is_forbidden_v4(v4: Ipv4Addr) -> bool {
    v4.is_loopback()
        || v4.is_private()
        || v4.is_link_local()
        || v4.is_unspecified()
        || v4.is_broadcast()
}

fn is_forbidden_v6(v6: Ipv6Addr) -> bool {
    if v6.is_loopback() || v6.is_unspecified() {
        return true;
    }
    if let Some(mapped) = v6.to_ipv4_mapped() {
        return is_forbidden_v4(mapped);
    }
    let seg0 = v6.segments()[0];
    // fc00::/7 (unique local, RFC 4193)
    if seg0 & 0xfe00 == 0xfc00 {
        return true;
    }
    // fe80::/10 (link-local)
    if seg0 & 0xffc0 == 0xfe80 {
        return true;
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_loopback_v4() {
        assert!(is_forbidden(IpAddr::V4(Ipv4Addr::new(127, 0, 0, 1))));
    }

    #[test]
    fn rejects_private_v4_ranges() {
        assert!(is_forbidden(IpAddr::V4(Ipv4Addr::new(10, 0, 0, 1))));
        assert!(is_forbidden(IpAddr::V4(Ipv4Addr::new(192, 168, 1, 1))));
        assert!(is_forbidden(IpAddr::V4(Ipv4Addr::new(172, 16, 0, 1))));
    }

    #[test]
    fn rejects_link_local_v4() {
        assert!(is_forbidden(IpAddr::V4(Ipv4Addr::new(169, 254, 1, 1))));
    }

    #[test]
    fn accepts_public_v4() {
        assert!(!is_forbidden(IpAddr::V4(Ipv4Addr::new(8, 8, 8, 8))));
        assert!(!is_forbidden(IpAddr::V4(Ipv4Addr::new(1, 1, 1, 1))));
    }

    #[test]
    fn rejects_loopback_and_unique_local_v6() {
        assert!(is_forbidden(IpAddr::V6(Ipv6Addr::LOCALHOST)));
        assert!(is_forbidden(IpAddr::V6(Ipv6Addr::new(
            0xfc00, 0, 0, 0, 0, 0, 0, 1
        ))));
        assert!(is_forbidden(IpAddr::V6(Ipv6Addr::new(
            0xfe80, 0, 0, 0, 0, 0, 0, 1
        ))));
    }

    #[test]
    fn accepts_public_v6() {
        // 2606:4700:4700::1111 (Cloudflare DNS)
        assert!(!is_forbidden(IpAddr::V6(Ipv6Addr::new(
            0x2606, 0x4700, 0x4700, 0, 0, 0, 0, 0x1111
        ))));
    }

    #[test]
    fn rejects_ipv4_mapped_private() {
        // ::ffff:192.168.1.1
        let v6 = Ipv4Addr::new(192, 168, 1, 1).to_ipv6_mapped();
        assert!(is_forbidden(IpAddr::V6(v6)));
    }

    // Los siguientes ejercitan `ensure_public_host` de punta a punta (incluida
    // la resolución vía `tokio::net::lookup_host`). Usan literales de IP a
    // propósito: resolver un literal numérico no necesita un DNS real
    // (getaddrinfo lo resuelve localmente), así que corren igual sin red.

    #[tokio::test]
    async fn ensure_public_host_rechaza_loopback() {
        let err = ensure_public_host("http://127.0.0.1:8080/algo")
            .await
            .unwrap_err();
        assert!(err.to_string().contains("privada/local"));
    }

    #[tokio::test]
    async fn ensure_public_host_rechaza_ip_privada() {
        assert!(ensure_public_host("http://192.168.1.1/").await.is_err());
        assert!(ensure_public_host("http://10.0.0.5/").await.is_err());
    }

    #[tokio::test]
    async fn ensure_public_host_rechaza_link_local() {
        assert!(ensure_public_host("http://169.254.1.1/").await.is_err());
    }

    #[tokio::test]
    async fn ensure_public_host_acepta_ip_publica() {
        assert!(ensure_public_host("http://1.1.1.1/").await.is_ok());
    }

    #[tokio::test]
    async fn ensure_public_host_rechaza_url_sin_host() {
        assert!(ensure_public_host("not a url").await.is_err());
    }
}
