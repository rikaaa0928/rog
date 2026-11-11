mod consts;
mod data;
mod default_router;
mod matcher;
pub(crate) mod remote_matcher;
pub(crate) mod resolver;
mod test;

use crate::def;
use crate::def::config::{RouteData, Router};
use crate::router::data::load_route_data;
use crate::router::default_router::DefaultBaseRouter;
use crate::router::matcher::Matcher;
use crate::router::resolver::Resolver;
use crate::util::RunAddr;
use std::collections::{HashMap, HashSet};
use std::sync::Arc;

// #[derive(Clone)]
pub struct DefaultRouter {
    router_map: HashMap<String, DefaultBaseRouter>,
    pub(crate) dns_table: Option<Arc<crate::dns::table::IpDomainTable>>,
    pub(crate) ebpf_direct_connectors: HashSet<String>,
    data_map: Arc<HashMap<String, Box<dyn Matcher>>>,
}

#[async_trait::async_trait]
impl def::RouterSet for DefaultRouter {
    async fn route_dns(
        &self,
        r_name: &str,
        domain: &str,
    ) -> std::io::Result<def::DnsRouteDecision> {
        self.router_map
            .get(r_name)
            .map(|r| r.route_dns(&crate::dns::table::canonical(domain)))
            .ok_or_else(|| {
                std::io::Error::new(std::io::ErrorKind::InvalidInput, "unknown DNS router")
            })
    }
    async fn route_with_context(
        &self,
        l_name: &str,
        r_name: &str,
        addr: &RunAddr,
        context: &def::RouteContext,
    ) -> String {
        self.route_with_domain(l_name, r_name, addr, context)
            .await
            .0
    }
    async fn route_with_domain(
        &self,
        l_name: &str,
        r_name: &str,
        addr: &RunAddr,
        context: &def::RouteContext,
    ) -> (String, Option<String>) {
        let proto = if addr.udp { "udp" } else { "tcp" };
        let src_ip = context
            .client_ip
            .map(|ip| ip.to_string())
            .unwrap_or_else(|| "-".to_string());
        let src_port = context
            .client_port
            .map(|p| p.to_string())
            .unwrap_or_else(|| "-".to_string());
        if context.use_dns_mapping {
            if let (Some(table), Some(client), Ok(target), Some(router)) = (
                &self.dns_table,
                context.client_ip,
                addr.addr.parse::<std::net::IpAddr>(),
                self.router_map.get(r_name),
            ) {
                let domains = table.get_domains(r_name, client, target);
                let (selected, matched_domain) =
                    router.route_mapped_with_domain(&addr.addr, &domains);
                if !l_name.is_empty() {
                    if !domains.is_empty() {
                        if self.ebpf_direct_connectors.contains(selected.as_str()) {
                            crate::util::ebpf::offload_direct_ip(
                                target,
                                std::time::Duration::from_secs(1800),
                            );
                        } else {
                            crate::util::ebpf::remove_direct_ip(target);
                        }
                    }
                    log::info!(
                        "Route listener: {} router: {} proto: {} src_ip: {} src_port: {} dst_ip: {} dst_port: {} domains: {:?} -> {}",
                        l_name,
                        r_name,
                        proto,
                        src_ip,
                        src_port,
                        addr.addr,
                        addr.port,
                        domains,
                        selected
                    );
                }
                return (selected, matched_domain);
            }
        }
        if let Some(router) = self.router_map.get(r_name) {
            let (match_name, res) = router.route(addr).await;
            if !l_name.is_empty() {
                log::info!(
                    "Route listener: {} router: {} proto: {} src_ip: {} src_port: {} dst_ip: {} dst_port: {} match: {} -> {}",
                    l_name,
                    r_name,
                    proto,
                    src_ip,
                    src_port,
                    addr.addr,
                    addr.port,
                    match_name,
                    res
                );
            }
            (res, None)
        } else {
            if !l_name.is_empty() {
                log::info!(
                    "Route listener: {} router: {} proto: {} src_ip: {} src_port: {} dst_ip: {} dst_port: {} -> {}",
                    l_name,
                    r_name,
                    proto,
                    src_ip,
                    src_port,
                    addr.addr,
                    addr.port,
                    "default by default"
                );
            }
            ("default".to_string(), None)
        }
    }

    async fn route(&self, l_name: &str, r_name: &str, addr: &RunAddr) -> String {
        self.route_with_domain(l_name, r_name, addr, &def::RouteContext::default())
            .await
            .0
    }
}

impl DefaultRouter {
    pub async fn new(cfg: &[Router], data_cfg: &[RouteData], resolver: Arc<Resolver>) -> Self {
        let data_map = Arc::new(load_route_data(data_cfg).await);
        let mut router_set = DefaultRouter {
            router_map: HashMap::new(),
            dns_table: None,
            ebpf_direct_connectors: HashSet::new(),
            data_map: data_map.clone(),
        };
        for r in cfg {
            let router = DefaultBaseRouter::new(
                r.name.clone(),
                r.default.clone(),
                r.route_rules.clone().unwrap_or(vec![]),
                data_map.clone(),
                resolver.clone(),
            );
            router_set.router_map.insert(r.name.clone(), router);
        }
        router_set
    }

    pub fn direct_cidrs(
        &self,
        r_name: &str,
        direct_connectors: &std::collections::HashSet<String>,
    ) -> Vec<ipnetwork::IpNetwork> {
        self.router_map
            .get(r_name)
            .map(|r| r.direct_cidrs(direct_connectors))
            .unwrap_or_default()
    }
}
