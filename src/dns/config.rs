use crate::def::config::Config;
use serde::Deserialize;
use std::{io, net::SocketAddr};

#[derive(Clone, Debug, Deserialize)]
#[serde(default)]
pub struct DnsConfig {
    pub endpoint: String,
    pub router: String,
    pub default_servers: Vec<String>,
    pub upstream_timeout_ms: u64,
    pub request_timeout_ms: u64,
    pub max_concurrent_requests: usize,
    pub mapping_capacity: usize,
    pub max_domains_per_ip: usize,
    pub mapping_max_ttl_secs: u64,
    pub mapping_cleanup_interval_secs: u64,
}
impl Default for DnsConfig {
    fn default() -> Self {
        Self {
            endpoint: String::new(),
            router: String::new(),
            default_servers: vec![],
            upstream_timeout_ms: 2000,
            request_timeout_ms: 6000,
            max_concurrent_requests: 256,
            mapping_capacity: 16384,
            max_domains_per_ip: 16,
            mapping_max_ttl_secs: 86400,
            mapping_cleanup_interval_secs: 30,
        }
    }
}
pub fn invalid(message: impl Into<String>) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidInput, message.into())
}
pub fn servers(values: &[String]) -> io::Result<Vec<SocketAddr>> {
    let mut result = vec![];
    for value in values {
        let addr: SocketAddr = value
            .parse()
            .map_err(|_| invalid(format!("invalid UDP DNS server: {value}")))?;
        if !addr.is_ipv4()
            || addr.port() == 0
            || addr.ip().is_unspecified()
            || addr.ip().is_multicast()
        {
            return Err(invalid(format!(
                "DNS upstream must be an IPv4 unicast address with nonzero port: {value}"
            )));
        }
        if !result.contains(&addr) {
            result.push(addr);
        }
    }
    Ok(result)
}
impl DnsConfig {
    pub fn validate(&self, config: &Config) -> io::Result<()> {
        let bind: SocketAddr = self
            .endpoint
            .parse()
            .map_err(|_| invalid("invalid DNS endpoint"))?;
        if bind.port() == 0 {
            return Err(invalid("DNS endpoint port must be nonzero"));
        }
        if self.default_servers.is_empty() {
            return Err(invalid("dns.default_servers must not be empty"));
        }
        if self.upstream_timeout_ms == 0
            || self.request_timeout_ms == 0
            || self.max_concurrent_requests == 0
            || self.mapping_capacity == 0
            || self.max_domains_per_ip == 0
            || self.mapping_max_ttl_secs == 0
            || self.mapping_cleanup_interval_secs == 0
        {
            return Err(invalid("DNS timeouts and capacities must be positive"));
        }
        if self.max_concurrent_requests > tokio::sync::Semaphore::MAX_PERMITS
            || self.upstream_timeout_ms > 86_400_000
            || self.request_timeout_ms > 86_400_000
            || self.mapping_cleanup_interval_secs > 86_400
            || self.mapping_max_ttl_secs > u32::MAX as u64
        {
            return Err(invalid(
                "DNS time or concurrency limit exceeds supported bounds",
            ));
        }
        let router = config
            .router
            .iter()
            .find(|r| r.name == self.router)
            .ok_or_else(|| invalid("dns.router does not exist"))?;
        for names in [
            config
                .router
                .iter()
                .map(|r| r.name.as_str())
                .collect::<Vec<_>>(),
            config
                .connector
                .iter()
                .map(|c| c.name.as_str())
                .collect::<Vec<_>>(),
            config
                .data
                .iter()
                .flatten()
                .map(|d| d.name.as_str())
                .collect::<Vec<_>>(),
        ] {
            let mut seen = std::collections::HashSet::new();
            if names.into_iter().any(|n| !seen.insert(n)) {
                return Err(invalid(
                    "duplicate router, connector or route data name in DNS configuration",
                ));
            }
        }
        let data: std::collections::HashSet<_> = config
            .data
            .iter()
            .flatten()
            .map(|d| d.name.as_str())
            .collect();
        let mut selected = vec![router.default.as_str()];
        for rule in router.route_rules.iter().flatten() {
            if !data.contains(rule.name.as_str()) {
                return Err(invalid(format!("unknown DNS route data: {}", rule.name)));
            }
            selected.push(rule.select.as_str());
        }
        let defaults = servers(&self.default_servers)?;
        for connector in &config.connector {
            if let Some(dns) = &connector.dns {
                servers(dns)?;
            }
        }
        for name in selected {
            let connector = config
                .connector
                .iter()
                .find(|c| c.name == name)
                .ok_or_else(|| invalid(format!("unknown DNS connector: {name}")))?;
            if !matches!(
                connector.proto.as_str(),
                "tcp" | "pb_tcp" | "grpc" | "rev_grpc" | "block"
            ) {
                return Err(invalid(format!(
                    "connector {name} does not support DNS UDP"
                )));
            }
            if matches!(connector.proto.as_str(), "pb_tcp" | "grpc") {
                if connector.pw.as_deref().is_none_or(str::is_empty) {
                    return Err(invalid(format!("DNS proxy {name} requires a password")));
                }
                let endpoint = connector
                    .endpoint
                    .as_deref()
                    .ok_or_else(|| invalid("DNS proxy requires an endpoint"))?;
                let numeric = endpoint.parse::<SocketAddr>().is_ok()
                    || url::Url::parse(endpoint)
                        .ok()
                        .and_then(|u| {
                            u.host_str().map(|h| {
                                h.trim_matches(['[', ']'])
                                    .parse::<std::net::IpAddr>()
                                    .is_ok()
                            })
                        })
                        .unwrap_or(false);
                if !numeric {
                    return Err(invalid(format!(
                        "DNS proxy {name} requires a numeric endpoint to avoid DNS bootstrap recursion"
                    )));
                }
            }
        }
        let mut all = defaults;
        for connector in &config.connector {
            if let Some(values) = &connector.dns {
                all.extend(servers(values)?);
            }
        }
        for addr in all {
            if addr.port() == bind.port()
                && (addr.ip() == bind.ip()
                    || (bind.ip().is_unspecified()
                        && crate::listener::tproxy::is_local_host_ip(&addr.ip()))
                    || (bind.ip().is_loopback() && addr.ip().is_loopback()))
            {
                return Err(invalid("DNS upstream points to the local DNS listener"));
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn sample_and_rejections() {
        let mut cfg: Config =
            toml::from_str(include_str!("../../dns_config_example.toml")).unwrap();
        cfg.validate().unwrap();
        cfg.dns.as_mut().unwrap().default_servers.clear();
        assert!(cfg.validate().is_err());
        cfg.dns.as_mut().unwrap().default_servers = vec!["127.0.0.1:5353".into()];
        assert!(cfg.validate().is_err());
        cfg.dns.as_mut().unwrap().default_servers = vec!["1.1.1.1:53".into()];
        cfg.dns.as_mut().unwrap().router = "missing".into();
        assert!(cfg.validate().is_err());
        assert!(servers(&["[::1]:53".into()]).is_err());
        assert!(servers(&["bad".into()]).is_err());
        assert_eq!(
            servers(&["1.1.1.1:53".into(), "1.1.1.1:53".into()])
                .unwrap()
                .len(),
            1
        );
    }
}
