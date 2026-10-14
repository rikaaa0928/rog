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
    pub buffer_size: Option<String>,
}

impl Config {
    pub fn apply_defaults(&mut self) {
        let default_servers = self
            .dns
            .as_ref()
            .map(|d| d.default_servers.clone())
            .filter(|v| !v.is_empty());
        for conn in &mut self.connector {
            if conn.dns.as_ref().is_none_or(Vec::is_empty) {
                if let Some(ref defaults) = default_servers {
                    conn.dns = Some(defaults.clone());
                }
            }
            let has_bootstrap = conn
                .options
                .as_ref()
                .is_some_and(|o| o.contains_key("bootstrap_dns"));
            if !has_bootstrap {
                if let Some(fallback_dns) = conn
                    .dns
                    .as_ref()
                    .filter(|v| !v.is_empty())
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
        Ok(())
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
    #[serde(default)]
    pub up: Option<String>,
    #[serde(default)]
    pub down: Option<String>,
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
