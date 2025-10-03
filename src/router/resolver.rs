use hickory_resolver::config::{NameServerConfig, ResolverConfig, ResolverOpts};
use hickory_resolver::proto::xfer::Protocol;
use hickory_resolver::{AsyncResolver, TokioAsyncResolver};
use std::collections::HashMap;
use std::net::IpAddr;
use std::sync::{Arc, RwLock};
use std::time::{Duration, Instant};

const CACHE_EXPIRATION: Duration = Duration::from_secs(3 * 60); // 3 minutes
const NEGATIVE_CACHE_EXPIRATION: Duration = Duration::from_secs(1 * 60); // 1 minute

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
        let mut interval = tokio::time::interval(Duration::from_secs(24 * 60 * 60));
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
        } else if dns_config.starts_with("doh://") {
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
        let async_resolver = TokioAsyncResolver::tokio(cfg, ResolverOpts::default());
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
        let async_resolver = AsyncResolver::tokio_from_system_conf().map_err(|e| e.to_string())?;
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
        // let dns_addr = dns_addr.strip_prefix("doh://").unwrap_or(dns_addr);
        // todo: doh
        let server_addr = self.resolve_ip_with_default_dns(addr).await;
        server_addr
    }
}
