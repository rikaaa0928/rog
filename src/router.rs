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
                let dns_domains = table.get_domains(r_name, client, target);
                let (selected, matched_domain) = if let Some(ref sniffed) = context.sniffed_domain {
                    if !dns_domains.iter().any(|d| d == sniffed) {
                        table.record(
                            r_name,
                            client,
                            target,
                            sniffed,
                            300,
                            std::time::Instant::now(),
                        );
                    }
                    router.route_mapped_with_domain(&addr.addr, std::slice::from_ref(sniffed))
                } else {
                    router.route_mapped_with_domain(&addr.addr, &dns_domains)
                };
                if !l_name.is_empty() {
                    if !dns_domains.is_empty()
                        || context.sniffed_domain.is_some()
                        || self.ebpf_direct_connectors.contains(selected.as_str())
                    {
                        self.sync_ebpf_direct_ip(
                            r_name,
                            target,
                            std::time::Duration::from_secs(1800),
                        );
                    }
                    if self.ebpf_direct_connectors.contains(selected.as_str()) {
                        crate::util::ebpf::note_direct_flow(
                            addr.udp,
                            client,
                            context.client_port.unwrap_or(0),
                            target,
                            addr.port,
                        );
                    }
                    if let Some(ref sniffed) = context.sniffed_domain {
                        log::info!(
                            "Route listener: {} router: {} proto: {} src_ip: {} src_port: {} dst_ip: {} dst_port: {} domains: {:?} sniffed: {} -> {}",
                            l_name,
                            r_name,
                            proto,
                            src_ip,
                            src_port,
                            addr.addr,
                            addr.port,
                            dns_domains,
                            sniffed,
                            selected
                        );
                    } else {
                        log::info!(
                            "Route listener: {} router: {} proto: {} src_ip: {} src_port: {} dst_ip: {} dst_port: {} domains: {:?} -> {}",
                            l_name,
                            r_name,
                            proto,
                            src_ip,
                            src_port,
                            addr.addr,
                            addr.port,
                            dns_domains,
                            selected
                        );
                    }
                }
                return (selected, matched_domain);
            }
        }
        if let Some(router) = self.router_map.get(r_name) {
            if let Some(ref sniffed) = context.sniffed_domain {
                let (selected, matched_domain) =
                    router.route_mapped_with_domain(&addr.addr, std::slice::from_ref(sniffed));
                if !l_name.is_empty() {
                    log::info!(
                        "Route listener: {} router: {} proto: {} src_ip: {} src_port: {} dst_ip: {} dst_port: {} sniffed: {} -> {}",
                        l_name,
                        r_name,
                        proto,
                        src_ip,
                        src_port,
                        addr.addr,
                        addr.port,
                        sniffed,
                        selected
                    );
                }
                return (selected, matched_domain);
            }
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

    fn sync_ebpf_direct_ip(
        &self,
        r_name: &str,
        target: std::net::IpAddr,
        ttl: std::time::Duration,
    ) {
        if self.ebpf_direct_connectors.is_empty() {
            return;
        }
        let Some(router) = self.router_map.get(r_name) else {
            return;
        };
        let all_domains = self
            .dns_table
            .as_ref()
            .map(|t| t.get_all_domains_for_ip(r_name, target))
            .unwrap_or_default();
        if router.allows_ebpf_direct_offload(
            &target.to_string(),
            &all_domains,
            &self.ebpf_direct_connectors,
        ) {
            crate::util::ebpf::offload_direct_ip(target, ttl);
        } else {
            crate::util::ebpf::remove_direct_ip(target);
        }
    }

    fn is_ambiguous_ip(
        &self,
        r_name: &str,
        client_ip: Option<std::net::IpAddr>,
        target_ip: &str,
    ) -> bool {
        let Ok(target) = target_ip.parse::<std::net::IpAddr>() else {
            return false;
        };
        let is_local_or_private = match target {
            std::net::IpAddr::V4(v4) => {
                v4.is_private() || v4.is_loopback() || v4.is_link_local() || v4.is_unspecified()
            }
            std::net::IpAddr::V6(v6) => {
                v6.is_loopback()
                    || v6.is_unspecified()
                    || v6.is_unicast_link_local()
                    || (v6.segments()[0] & 0xfe00) == 0xfc00
            }
        };
        let Some(router) = self.router_map.get(r_name) else {
            return false;
        };
        if router.has_ip_exclude_match(target_ip) {
            return true;
        }
        let Some(table) = &self.dns_table else {
            return !is_local_or_private;
        };
        let client_domains = client_ip
            .map(|c| table.get_domains(r_name, c, target))
            .unwrap_or_default();
        let all_domains = table.get_all_domains_for_ip(r_name, target);
        if is_local_or_private && all_domains.is_empty() && client_domains.is_empty() {
            return false;
        }
        if client_domains.is_empty() {
            return true;
        }
        let check_domains = if all_domains.is_empty() {
            &client_domains
        } else {
            &all_domains
        };
        let mut first_route: Option<String> = None;
        for domain in check_domains {
            let (route, _) =
                router.route_mapped_with_domain(target_ip, std::slice::from_ref(domain));
            match &first_route {
                None => first_route = Some(route),
                Some(prev) if prev != &route => return true,
                _ => {}
            }
        }
        false
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

    pub(crate) fn lookup_ebpf_direct_route(
        &self,
        r_name: &str,
        client_ip: std::net::IpAddr,
        target_ip: std::net::IpAddr,
    ) -> (String, Vec<String>, Option<String>) {
        let domains = self
            .dns_table
            .as_ref()
            .map(|t| t.get_domains(r_name, client_ip, target_ip))
            .unwrap_or_default();
        let target_str = target_ip.to_string();
        if let Some(router) = self.router_map.get(r_name) {
            let (selected, matched_domain) = router.route_mapped_with_domain(&target_str, &domains);
            (selected, domains, matched_domain)
        } else {
            let fallback = domains.first().cloned();
            ("default".to_string(), domains, fallback)
        }
    }
}

#[cfg(test)]
mod ebpf_tests {
    use super::*;
    use crate::def::config::{RouteData, RouteRule, Router};
    use crate::dns::table::IpDomainTable;
    use std::net::IpAddr;
    use std::time::Instant;

    #[tokio::test]
    async fn shared_ip_across_clients_blocks_ebpf_while_keeping_client_routing() {
        use crate::def::RouterSet;
        let resolver = Resolver::new();
        let data_cfg = vec![
            RouteData {
                name: "llm".into(),
                url: None,
                format: "domain-suffix".into(),
                data: Some("llm.rikaaa0928.moe".into()),
                interval: None,
            },
            RouteData {
                name: "infra".into(),
                url: None,
                format: "domain-suffix".into(),
                data: Some("ip.rikaaa0928.moe".into()),
                interval: None,
            },
        ];
        let router_cfg = vec![Router {
            name: "main".into(),
            default: "ROGJP".into(),
            route_rules: Some(vec![
                RouteRule {
                    name: "llm".into(),
                    select: "ROGJPH3".into(),
                    exclude: vec![],
                    domain_to_ip: None,
                    dns: None,
                },
                RouteRule {
                    name: "infra".into(),
                    select: "Direct".into(),
                    exclude: vec![],
                    domain_to_ip: None,
                    dns: None,
                },
            ]),
        }];
        let mut router = DefaultRouter::new(&router_cfg, &data_cfg, resolver).await;
        let table = Arc::new(IpDomainTable::new(64, 8, 3600));
        router.dns_table = Some(table.clone());
        router.ebpf_direct_connectors = ["Direct".into()].into_iter().collect();

        let shared_ip: IpAddr = "141.147.149.97".parse().unwrap();
        let c1: IpAddr = "192.168.0.48".parse().unwrap();
        let c2: IpAddr = "192.168.0.121".parse().unwrap();
        let pure_direct_ip: IpAddr = "1.2.3.4".parse().unwrap();
        let now = Instant::now();

        table.record("main", c1, shared_ip, "llm.rikaaa0928.moe", 60, now);
        table.record(
            "main",
            c2,
            shared_ip,
            "ragents-api.ip.rikaaa0928.moe",
            60,
            now,
        );
        table.record(
            "main",
            c2,
            pure_direct_ip,
            "direct.ip.rikaaa0928.moe",
            60,
            now,
        );

        // Client-level user-space routing remains intact.
        assert_eq!(
            router
                .route_with_domain(
                    "tproxy",
                    "main",
                    &RunAddr {
                        addr: shared_ip.to_string(),
                        port: 443,
                        udp: false,
                    },
                    &def::RouteContext {
                        client_ip: Some(c1),
                        client_port: Some(50001),
                        use_dns_mapping: true,
                        sniffed_domain: None,
                    },
                )
                .await
                .0,
            "ROGJPH3"
        );
        assert_eq!(
            router
                .route_with_domain(
                    "tproxy",
                    "main",
                    &RunAddr {
                        addr: shared_ip.to_string(),
                        port: 443,
                        udp: false,
                    },
                    &def::RouteContext {
                        client_ip: Some(c2),
                        client_port: Some(50002),
                        use_dns_mapping: true,
                        sniffed_domain: None,
                    },
                )
                .await
                .0,
            "Direct"
        );

        // Global eBPF decision rejects the shared IP because c1 maps it to ROGJPH3,
        // while allowing the pure Direct IP.
        let shared_all = table.get_all_domains_for_ip("main", shared_ip);
        assert!(!router
            .router_map
            .get("main")
            .unwrap()
            .allows_ebpf_direct_offload(
                &shared_ip.to_string(),
                &shared_all,
                &router.ebpf_direct_connectors
            ));
        let pure_all = table.get_all_domains_for_ip("main", pure_direct_ip);
        assert!(router
            .router_map
            .get("main")
            .unwrap()
            .allows_ebpf_direct_offload(
                &pure_direct_ip.to_string(),
                &pure_all,
                &router.ebpf_direct_connectors
            ));

        // Ambiguity & SNI override verification:
        // Shared IP is ambiguous; pure direct IP and LAN private IP are not.
        assert!(router.is_ambiguous_ip("main", Some(c1), &shared_ip.to_string()));
        assert!(router.is_ambiguous_ip("main", Some(c2), &shared_ip.to_string()));
        assert!(!router.is_ambiguous_ip("main", Some(c2), &pure_direct_ip.to_string()));
        assert!(!router.is_ambiguous_ip("main", Some(c2), "192.168.0.1"));

        // Even when the same client has both Direct and proxy domains cached on shared_ip,
        // sniffed_domain selects the exact outbound for that TLS connection.
        table.record("main", c2, shared_ip, "llm.rikaaa0928.moe", 600, now);
        assert_eq!(
            router
                .route_with_domain(
                    "tproxy",
                    "main",
                    &RunAddr {
                        addr: shared_ip.to_string(),
                        port: 443,
                        udp: false,
                    },
                    &def::RouteContext {
                        client_ip: Some(c2),
                        client_port: Some(50003),
                        use_dns_mapping: true,
                        sniffed_domain: Some("llm.rikaaa0928.moe".to_string()),
                    },
                )
                .await,
            ("ROGJPH3".to_string(), Some("llm.rikaaa0928.moe".to_string()))
        );
        assert_eq!(
            router
                .route_with_domain(
                    "tproxy",
                    "main",
                    &RunAddr {
                        addr: shared_ip.to_string(),
                        port: 443,
                        udp: false,
                    },
                    &def::RouteContext {
                        client_ip: Some(c2),
                        client_port: Some(50004),
                        use_dns_mapping: true,
                        sniffed_domain: Some("ragents-api.ip.rikaaa0928.moe".to_string()),
                    },
                )
                .await,
            (
                "Direct".to_string(),
                Some("ragents-api.ip.rikaaa0928.moe".to_string())
            )
        );
    }

    #[tokio::test]
    async fn excluded_ip_always_triggers_sniff_even_with_single_direct_domain() {
        use crate::def::RouterSet;
        let resolver = Resolver::new();
        let data_cfg = vec![
            RouteData {
                name: "vps_cidr".into(),
                url: None,
                format: "cidr".into(),
                data: Some("141.147.149.97/32\n1.2.3.4/32".into()),
                interval: None,
            },
            RouteData {
                name: "llm".into(),
                url: None,
                format: "domain-suffix".into(),
                data: Some("llm.rikaaa0928.moe".into()),
                interval: None,
            },
        ];
        let router_cfg = vec![Router {
            name: "main".into(),
            default: "ROGJP".into(),
            route_rules: Some(vec![
                RouteRule {
                    name: "llm".into(),
                    select: "ROGJPH3".into(),
                    exclude: vec![],
                    domain_to_ip: None,
                    dns: None,
                },
                RouteRule {
                    name: "vps_cidr".into(),
                    select: "Direct".into(),
                    exclude: vec!["141.147.149.97/32".into()],
                    domain_to_ip: None,
                    dns: None,
                },
            ]),
        }];
        let mut router = DefaultRouter::new(&router_cfg, &data_cfg, resolver).await;
        let table = Arc::new(IpDomainTable::new(64, 8, 3600));
        router.dns_table = Some(table.clone());

        let c1: IpAddr = "192.168.0.48".parse().unwrap();
        let now = Instant::now();
        table.record(
            "main",
            c1,
            "141.147.149.97".parse().unwrap(),
            "other.rikaaa0928.moe",
            600,
            now,
        );
        table.record(
            "main",
            c1,
            "1.2.3.4".parse().unwrap(),
            "pure.rikaaa0928.moe",
            600,
            now,
        );

        // Even though dns_table only knows one domain on 141.147.149.97,
        // the exclude rule marks it for SNI sniffing while leaving 1.2.3.4 un-sniffed.
        assert!(router.is_ambiguous_ip("main", Some(c1), "141.147.149.97"));
        assert!(!router.is_ambiguous_ip("main", Some(c1), "1.2.3.4"));
    }
}
