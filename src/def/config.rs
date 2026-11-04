use serde::Deserialize;
use std::collections::HashMap;

#[derive(Deserialize, Debug, Clone)]
pub struct Config {
    pub dns: Option<crate::dns::config::DnsConfig>,
    pub reverse_server: Option<ReverseServer>,
    pub listener: Vec<Listener>,
    pub router: Vec<Router>,
    pub data: Option<Vec<RouteData>>,
    pub connector: Vec<Connector>,
    pub server_id: Option<String>,
}

impl Config {
    pub fn apply_defaults(&mut self) {
        let default_servers = self
            .dns
            .as_ref()
            .map(|d| d.default_servers.clone())
            .filter(|v| !v.is_empty());
        let bootstrap_servers = self
            .dns
            .as_ref()
            .map(|d| d.bootstrap_servers.clone())
            .filter(|v| !v.is_empty());
        for conn in &mut self.connector {
            let explicit_conn_dns = conn.dns.clone().filter(|v| !v.is_empty());
            if explicit_conn_dns.is_none()
                && let Some(ref defaults) = default_servers
            {
                conn.dns = Some(defaults.clone());
            }
            let has_bootstrap = conn
                .options
                .as_ref()
                .is_some_and(|o| o.contains_key("bootstrap_dns"));
            if !has_bootstrap
                && let Some(fallback_dns) = bootstrap_servers
                    .as_ref()
                    .or(explicit_conn_dns.as_ref())
                    .or(default_servers.as_ref())
            {
                let bootstrap_val = toml::Value::Array(
                    fallback_dns
                        .iter()
                        .cloned()
                        .map(toml::Value::String)
                        .collect(),
                );
                conn.options
                    .get_or_insert_with(HashMap::new)
                    .insert("bootstrap_dns".to_string(), bootstrap_val);
            }
        }
    }

    pub fn validate(&self) -> std::io::Result<()> {
        if let Some(dns) = &self.dns {
            dns.validate(self)?;
        }
        if self
            .listener
            .iter()
            .filter(|listener| listener.proto.eq_ignore_ascii_case("tproxy"))
            .count()
            > 1
        {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "only one tproxy inbound listener is allowed per configuration; use mode = \"tcp+udp\" for both protocols",
            ));
        }
        for name in self.explicit_ebpf_direct_connectors() {
            if !self.connector.iter().any(|c| c.name == name) {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::InvalidInput,
                    format!("unknown ebpf_direct_connectors connector: {name}"),
                ));
            }
        }
        Ok(())
    }

    pub fn explicit_ebpf_direct_connectors(&self) -> Vec<String> {
        let mut names = Vec::new();
        if let Some(dns) = &self.dns {
            for name in &dns.ebpf_direct_connectors {
                let trimmed = name.trim();
                if !trimmed.is_empty() && !names.iter().any(|n| n == trimmed) {
                    names.push(trimmed.to_string());
                }
            }
        }
        for listener in self
            .listener
            .iter()
            .filter(|l| l.proto.eq_ignore_ascii_case("tproxy"))
        {
            let opts = crate::listener::tproxy::TproxyOptions::from_listener(listener);
            for name in opts.ebpf_direct_connectors {
                if !names.iter().any(|n| n == &name) {
                    names.push(name);
                }
            }
        }
        names
    }

    pub fn resolve_ebpf_direct_connectors(&self) -> std::collections::HashSet<String> {
        let explicit = self.explicit_ebpf_direct_connectors();
        if !explicit.is_empty() {
            return explicit.into_iter().collect();
        }
        self.connector
            .iter()
            .filter(|c| c.proto == "tcp")
            .map(|c| c.name.clone())
            .collect()
    }
}

#[derive(Deserialize, Debug, Clone)]
pub struct ReverseServer {
    pub endpoint: String,
    pub options: Option<HashMap<String, toml::Value>>,
}

#[derive(Deserialize, Debug, Clone)]
pub struct Connector {
    pub dns: Option<Vec<String>>,
    pub endpoint: Option<String>,
    pub name: String,
    pub user: Option<String>,
    pub pw: Option<String>,
    pub proto: String,
    pub options: Option<HashMap<String, toml::Value>>,
}

#[derive(Deserialize, Debug, Clone)]
pub struct Router {
    pub name: String,
    pub default: String,
    pub route_rules: Option<Vec<RouteRule>>,
}

#[derive(Deserialize, Debug, Clone)]
pub struct Listener {
    pub endpoint: String,
    pub name: String,
    pub user: Option<String>,
    pub pw: Option<String>,
    pub proto: String,
    pub router: String,
    pub options: Option<HashMap<String, toml::Value>>,
}

#[derive(Deserialize, Debug, Clone)]
pub struct RouteRule {
    pub name: String,
    pub select: String,
    pub exclude: Vec<String>,
    pub domain_to_ip: Option<bool>,
    pub dns: Option<String>,
}

#[derive(Deserialize, Debug, Clone)]
pub struct RouteData {
    pub name: String,
    pub url: Option<String>,
    pub format: String,
    pub data: Option<String>,
    pub interval: Option<u64>,
}

#[cfg(test)]
mod tests {
    use crate::load_config_str;

    #[test]
    fn test_resolve_ebpf_direct_connectors() {
        let cfg = load_config_str(
            r#"
[dns]
endpoint = "127.0.0.1:5353"
router = "main"
default_servers = ["1.1.1.1:53"]

[[listener]]
name = "tp"
endpoint = "[::]:12345"
proto = "tproxy"
router = "main"

[[connector]]
name = "DirectA"
proto = "tcp"

[[connector]]
name = "DirectB"
proto = "tcp"

[[router]]
name = "main"
default = "DirectA"
"#,
        )
        .unwrap();
        let resolved = cfg.resolve_ebpf_direct_connectors();
        assert_eq!(resolved.len(), 2);
        assert!(resolved.contains("DirectA"));
        assert!(resolved.contains("DirectB"));

        let cfg_explicit = load_config_str(
            r#"
[dns]
endpoint = "127.0.0.1:5353"
router = "main"
default_servers = ["1.1.1.1:53"]
ebpf_direct_connectors = ["DirectA"]

[[listener]]
name = "tp"
endpoint = "[::]:12345"
proto = "tproxy"
router = "main"

[[connector]]
name = "DirectA"
proto = "tcp"

[[connector]]
name = "DirectB"
proto = "tcp"

[[router]]
name = "main"
default = "DirectA"
"#,
        )
        .unwrap();
        let resolved_explicit = cfg_explicit.resolve_ebpf_direct_connectors();
        assert_eq!(resolved_explicit.len(), 1);
        assert!(resolved_explicit.contains("DirectA"));
        assert!(!resolved_explicit.contains("DirectB"));

        let err = load_config_str(
            r#"
[dns]
endpoint = "127.0.0.1:5353"
router = "main"
default_servers = ["1.1.1.1:53"]
ebpf_direct_connectors = ["NonExistent"]

[[listener]]
name = "tp"
endpoint = "[::]:12345"
proto = "tproxy"
router = "main"

[[connector]]
name = "DirectA"
proto = "tcp"

[[router]]
name = "main"
default = "DirectA"
"#,
        )
        .unwrap_err();
        assert!(
            err.to_string()
                .contains("unknown ebpf_direct_connectors connector: NonExistent")
        );
    }
}
