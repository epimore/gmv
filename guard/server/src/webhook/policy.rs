use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};

use url::Url;

use crate::core::{GuardError, GuardResult};

#[derive(Debug, Clone, Default)]
pub struct WebhookUrlPolicy {
    pub private_network_allowlist: Vec<String>,
}

impl WebhookUrlPolicy {
    pub async fn resolve(&self, url: &Url) -> GuardResult<Vec<SocketAddr>> {
        if !matches!(url.scheme(), "http" | "https") {
            return Err(GuardError::InvalidConfig(
                "webhook URL must use HTTP or HTTPS".to_string(),
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
        let disallowed = if url.scheme() == "http" {
            addresses
                .iter()
                .any(|address| !self.address_allowlisted(host, address.ip()))
        } else {
            addresses.iter().any(|address| {
                !is_public(address.ip()) && !self.address_allowlisted(host, address.ip())
            })
        };
        if disallowed {
            return Err(GuardError::InvalidIdentity(
                "webhook host is not allowed by the URL policy".to_string(),
            ));
        }
        Ok(addresses)
    }

    fn address_allowlisted(&self, host: &str, ip: IpAddr) -> bool {
        self.private_network_allowlist.iter().any(|entry| {
            entry.eq_ignore_ascii_case(host)
                || entry.parse::<IpAddr>().is_ok_and(|allowed| allowed == ip)
                || entry
                    .parse::<ipnet::IpNet>()
                    .is_ok_and(|network| network.contains(&ip))
        })
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

    #[test]
    fn allowlist_accepts_exact_host_ip_or_cidr_without_network_classification() {
        let policy = WebhookUrlPolicy {
            private_network_allowlist: vec![
                "10.20.0.0/16".to_string(),
                "partner.internal".to_string(),
            ],
        };
        assert!(policy.address_allowlisted("other.internal", "10.20.1.2".parse().unwrap()));
        assert!(policy.address_allowlisted("partner.internal", "192.168.1.3".parse().unwrap()));
        assert!(policy.address_allowlisted("partner.internal", "127.0.0.1".parse().unwrap()));
        assert!(!policy.address_allowlisted("other.internal", "192.168.1.3".parse().unwrap()));

        let loopback_policy = WebhookUrlPolicy {
            private_network_allowlist: vec![
                "127.0.0.1".to_string(),
                "localhost".to_string(),
                "127.0.0.0/8".to_string(),
            ],
        };
        assert!(loopback_policy.address_allowlisted("127.0.0.1", "127.0.0.1".parse().unwrap()));
        assert!(loopback_policy.address_allowlisted("localhost", "::1".parse().unwrap()));
        assert!(
            WebhookUrlPolicy {
                private_network_allowlist: vec!["127.0.0.0/8".to_string()]
            }
            .address_allowlisted("127.0.0.1", "127.0.0.1".parse().unwrap())
        );
    }

    #[test]
    fn http_requires_an_explicit_allowlist_match() {
        base::tokio::runtime::Runtime::new()
            .unwrap()
            .block_on(async {
                let private = Url::parse("http://192.168.0.8:9000/events").unwrap();
                assert!(WebhookUrlPolicy::default().resolve(&private).await.is_err());
                assert!(
                    WebhookUrlPolicy {
                        private_network_allowlist: vec!["192.168.0.8".to_string()]
                    }
                    .resolve(&private)
                    .await
                    .is_ok()
                );

                let loopback = Url::parse("http://127.0.0.1:9000/events").unwrap();
                assert!(
                    WebhookUrlPolicy::default()
                        .resolve(&loopback)
                        .await
                        .is_err()
                );
                assert!(
                    WebhookUrlPolicy {
                        private_network_allowlist: vec!["127.0.0.1".to_string()]
                    }
                    .resolve(&loopback)
                    .await
                    .is_ok()
                );

                let public = Url::parse("http://8.8.8.8/events").unwrap();
                assert!(
                    WebhookUrlPolicy {
                        private_network_allowlist: vec!["8.8.8.8".to_string()]
                    }
                    .resolve(&public)
                    .await
                    .is_ok()
                );

                let link_local = Url::parse("http://169.254.169.254/latest/meta-data").unwrap();
                assert!(
                    WebhookUrlPolicy {
                        private_network_allowlist: vec!["169.254.169.254".to_string()]
                    }
                    .resolve(&link_local)
                    .await
                    .is_ok()
                );
            });
    }
}
