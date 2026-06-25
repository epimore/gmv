use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};

use url::Url;

use crate::core::{GuardError, GuardResult};

#[derive(Debug, Clone, Default)]
pub struct WebhookUrlPolicy {
    pub allow_private_networks: bool,
}

impl WebhookUrlPolicy {
    pub async fn resolve(&self, url: &Url) -> GuardResult<Vec<SocketAddr>> {
        if url.scheme() != "https" {
            return Err(GuardError::InvalidConfig(
                "webhook URL must use HTTPS".to_string(),
            ));
        }
        if !url.username().is_empty() || url.password().is_some() {
            return Err(GuardError::InvalidConfig(
                "webhook URL must not contain credentials".to_string(),
            ));
        }
        let host = url
            .host_str()
            .ok_or_else(|| GuardError::InvalidConfig("webhook URL host is required".to_string()))?;
        if host.eq_ignore_ascii_case("localhost") {
            return Err(GuardError::InvalidIdentity(
                "webhook localhost is not allowed".to_string(),
            ));
        }
        let port = url
            .port_or_known_default()
            .ok_or_else(|| GuardError::InvalidConfig("webhook URL port is required".to_string()))?;
        let addresses = base::tokio::net::lookup_host((host, port))
            .await
            .map_err(|error| GuardError::Conflict(format!("webhook DNS failed: {error}")))?
            .collect::<Vec<_>>();
        if addresses.is_empty() {
            return Err(GuardError::NotFound(
                "webhook host resolved to no addresses".to_string(),
            ));
        }
        if !self.allow_private_networks && addresses.iter().any(|address| !is_public(address.ip()))
        {
            return Err(GuardError::InvalidIdentity(
                "webhook host resolves to a non-public address".to_string(),
            ));
        }
        Ok(addresses)
    }
}

fn is_public(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(ip) => is_public_v4(ip),
        IpAddr::V6(ip) => is_public_v6(ip),
    }
}

fn is_public_v4(ip: Ipv4Addr) -> bool {
    let octets = ip.octets();
    !(ip.is_private()
        || ip.is_loopback()
        || ip.is_link_local()
        || ip.is_multicast()
        || ip.is_unspecified()
        || octets[0] == 0
        || octets[0] >= 224
        || (octets[0] == 100 && (64..=127).contains(&octets[1]))
        || (octets[0] == 192 && octets[1] == 0 && octets[2] == 2)
        || (octets[0] == 198 && octets[1] == 51 && octets[2] == 100)
        || (octets[0] == 203 && octets[1] == 0 && octets[2] == 113))
}

fn is_public_v6(ip: Ipv6Addr) -> bool {
    if let Some(v4) = ip.to_ipv4_mapped() {
        return is_public_v4(v4);
    }
    let segments = ip.segments();
    !(ip.is_loopback()
        || ip.is_unspecified()
        || ip.is_multicast()
        || (segments[0] & 0xfe00) == 0xfc00
        || (segments[0] & 0xffc0) == 0xfe80
        || (segments[0] == 0x2001 && segments[1] == 0x0db8))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_private_and_documentation_addresses() {
        for value in [
            "127.0.0.1",
            "10.0.0.1",
            "169.254.1.1",
            "192.0.2.1",
            "::1",
            "fd00::1",
            "2001:db8::1",
        ] {
            assert!(!is_public(value.parse().unwrap()), "{value}");
        }
        assert!(is_public("8.8.8.8".parse().unwrap()));
        assert!(is_public("2606:4700:4700::1111".parse().unwrap()));
    }
}
