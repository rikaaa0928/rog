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
use std::collections::HashMap;
use std::sync::Arc;

// #[derive(Clone)]
pub struct DefaultRouter {
    router_map: HashMap<String, DefaultBaseRouter>,
    pub(crate) dns_table: Option<Arc<crate::dns::table::IpDomainTable>>,
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
                log::info!(
                    "Route listener: {} router: {} ip: {} domains: {:?} -> {}",
                    l_name,
                    r_name,
                    addr.addr,
                    domains,
                    selected
                );
                return (selected, matched_domain);
            }
        }
        (self.route(l_name, r_name, addr).await, None)
    }

    async fn route(&self, l_name: &str, r_name: &str, addr: &RunAddr) -> String {
        if let Some(router) = self.router_map.get(r_name) {
            let (match_name, res) = router.route(addr).await;
            log::info!(
                "Route listener: {} router: {} addr: {} match: {} -> {}",
                l_name,
                r_name,
                addr.addr,
                match_name,
                res
            );
            res
        } else {
            log::info!(
                "Route {} {} {} -> {}",
                l_name,
                r_name,
                addr.addr,
                "default by default"
            );
            "default".to_string()
        }
    }
}

impl DefaultRouter {
    pub async fn new(cfg: &[Router], data_cfg: &[RouteData], resolver: Arc<Resolver>) -> Self {
        let data_map = Arc::new(load_route_data(data_cfg).await);
        let mut router_set = DefaultRouter {
            router_map: HashMap::new(),
            dns_table: None,
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
}
