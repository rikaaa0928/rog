use hickory_resolver::config::{NameServerConfig, ResolverConfig};
use hickory_resolver::name_server::TokioConnectionProvider;
use hickory_resolver::proto::xfer::Protocol;
use std::collections::HashMap;
use std::net::{IpAddr, SocketAddr};
use std::sync::{Arc, RwLock};
use std::time::{Duration, Instant};
use url::Url;

#[cfg(not(test))]
const CACHE_EXPIRATION: Duration = Duration::from_secs(3 * 60); // 3 minutes
#[cfg(test)]
const CACHE_EXPIRATION: Duration = Duration::from_millis(50);

#[cfg(not(test))]
const NEGATIVE_CACHE_EXPIRATION: Duration = Duration::from_secs(60); // 1 minute
#[cfg(test)]
const NEGATIVE_CACHE_EXPIRATION: Duration = Duration::from_millis(50);

#[cfg(not(test))]
const CACHE_CLEANUP_INTERVAL: Duration = Duration::from_secs(24 * 60 * 60);
#[cfg(test)]
const CACHE_CLEANUP_INTERVAL: Duration = Duration::from_millis(25);

#[derive(Debug, Clone)]
pub(crate) struct ResolveResult {
    ips: Vec<IpAddr>,
    expiry: Instant,
    err: Option<String>,
}
pub struct Resolver {
    pub(crate) cache: Arc<RwLock<HashMap<String, ResolveResult>>>,
}

impl Resolver {
    pub fn new() -> Arc<Self> {
        let resolver = Arc::new(Resolver {
            cache: Arc::new(RwLock::new(HashMap::new())),
        });
        // 启动定时清理缓存的 task
        tokio::spawn(resolver.clone().start_cache_cleanup());
        resolver
    }

    async fn start_cache_cleanup(self: Arc<Self>) {
        let mut interval = tokio::time::interval(CACHE_CLEANUP_INTERVAL);
        loop {
            interval.tick().await;
            self.cleanup_cache();
        }
    }

    fn cleanup_cache(&self) {
        let now = Instant::now();
        let mut cache = self.cache.write().unwrap();
        cache.retain(|_key, res| now.lt(&res.expiry));
        log::info!("DNS cache cleanup finished");
    }

    pub async fn resolve_ip(&self, addr: &str, dns_config: &str) -> Result<Vec<IpAddr>, String> {
        let cache_key = format!("{}-{}", addr, dns_config);
        if let Some(cached) = self.cache.read().unwrap().get(&cache_key) {
            if Instant::now().lt(&cached.expiry) {
                return if cached.err.is_some() {
                    Err(cached.err.clone().unwrap())
                } else {
                    Ok(cached.ips.clone())
                };
            }
        }

        let result = if dns_config.is_empty() {
            self.resolve_ip_with_default_dns(addr).await
        } else if dns_config.starts_with("doh://") || dns_config.starts_with("https://") {
            self.resolve_ip_with_doh(addr, dns_config).await
        } else {
            self.resolve_ip_with_specific_dns(addr, dns_config).await
        };

        let expiry = Instant::now() + self.get_cache_expiration(result.is_err());
        let res = ResolveResult {
            ips: result.as_ref().unwrap_or(&Vec::new()).clone(),
            expiry,
            err: result.clone().err().map(|e| e.to_string()),
        };
        self.cache.write().unwrap().insert(cache_key, res);
        result
    }

    fn get_cache_expiration(&self, is_error: bool) -> Duration {
        if is_error {
            NEGATIVE_CACHE_EXPIRATION
        } else {
            CACHE_EXPIRATION
        }
    }

    pub(crate) async fn resolve_ip_with_specific_dns(
        &self,
        addr: &str,
        dns_addr: &str,
    ) -> Result<Vec<IpAddr>, String> {
        let server_addr = match dns_addr.parse() {
            Ok(sa) => sa,
            Err(e) => return Err(format!("invalid DNS server address: {}", e)),
        };
        let mut cfg = ResolverConfig::new();
        cfg.add_name_server(NameServerConfig {
            socket_addr: server_addr,
            protocol: Protocol::default(),
            tls_dns_name: None,
            http_endpoint: None,
            trust_negative_responses: false,
            bind_addr: None,
        });
        let async_resolver = hickory_resolver::Resolver::builder_with_config(
            cfg,
            TokioConnectionProvider::default(),
        )
        .build();
        let response = async_resolver
            .lookup_ip(addr)
            .await
            .map_err(|e| e.to_string())?;
        Ok(response.iter().collect())
    }

    pub(crate) async fn resolve_ip_with_default_dns(
        &self,
        addr: &str,
    ) -> Result<Vec<IpAddr>, String> {
        let async_resolver = hickory_resolver::Resolver::builder_tokio().unwrap().build();
        let response = async_resolver
            .lookup_ip(addr)
            .await
            .map_err(|e| e.to_string())?;
        Ok(response.iter().collect())
    }

    pub(crate) async fn resolve_ip_with_doh(
        &self,
        addr: &str,
        dns_addr: &str,
    ) -> Result<Vec<IpAddr>, String> {
        let (host, endpoint, port) = parse_doh_config(dns_addr)?;
        let socket_addr = if let Ok(ip) = host.parse::<IpAddr>() {
            SocketAddr::new(ip, port)
        } else {
            let ips = self.resolve_ip_with_default_dns(&host).await?;
            let ip = ips
                .into_iter()
                .next()
                .ok_or_else(|| format!("no IP found for DoH server: {}", host))?;
            SocketAddr::new(ip, port)
        };

        let mut cfg = ResolverConfig::new();
        cfg.add_name_server(NameServerConfig {
            socket_addr,
            protocol: Protocol::Https,
            tls_dns_name: Some(host),
            http_endpoint: Some(endpoint),
            trust_negative_responses: false,
            bind_addr: None,
        });
        let async_resolver = hickory_resolver::Resolver::builder_with_config(
            cfg,
            TokioConnectionProvider::default(),
        )
        .build();
        let response = async_resolver
            .lookup_ip(addr)
            .await
            .map_err(|e| e.to_string())?;
        Ok(response.iter().collect())
    }
}

fn parse_doh_config(dns_addr: &str) -> Result<(String, String, u16), String> {
    let url = if let Some(rest) = dns_addr.strip_prefix("doh://") {
        Url::parse(&format!("https://{}", rest))
    } else {
        Url::parse(dns_addr)
    }
    .map_err(|e| format!("invalid DoH URL '{}': {}", dns_addr, e))?;

    let host = url
        .host_str()
        .ok_or_else(|| format!("invalid DoH URL '{}': missing host", dns_addr))?
        .to_string();
    let port = url.port_or_known_default().unwrap_or(443);
    let endpoint = if url.path().is_empty() || url.path() == "/" {
        "/dns-query".to_string()
    } else {
        url.path().to_string()
    };
    Ok((host, endpoint, port))
}
