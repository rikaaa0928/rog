use crate::def::config;
use crate::def::config::RouteRule;
use crate::router::matcher::util::match_exclude;
use crate::router::matcher::Matcher;
use crate::router::resolver::Resolver;
use crate::util::RunAddr;
use log::warn;
use std::collections::HashMap;
use std::sync::Arc;

pub struct DefaultBaseRouter {
    pub name: String,
    pub default_tag: String,
    pub rules: Vec<RouteRule>,
    pub data_map: Arc<HashMap<String, Box<dyn Matcher>>>,
    pub resolver: Arc<Resolver>,
}

impl DefaultBaseRouter {
    pub fn new(
        name: String,
        default_tag: String,
        rules: Vec<RouteRule>,
        data_map: Arc<HashMap<String, Box<dyn Matcher>>>,
        resolver: Arc<Resolver>,
    ) -> Self {
        DefaultBaseRouter {
            name,
            default_tag,
            rules,
            data_map,
            resolver,
        }
    }

    pub(crate) async fn route(&self, addr: &RunAddr) -> (String, String) {
        for rule in &self.rules {
            let mut hosts = vec![addr.addr.clone().to_string()];
            if rule.domain_to_ip.is_some() && rule.domain_to_ip.unwrap() {
                match self
                    .resolver
                    .resolve_ip(addr.addr.as_str(), rule.dns.as_ref().unwrap())
                    .await
                {
                    Ok(ips) => {
                        for ip in ips {
                            let ip_str = ip.to_string();
                            if !hosts.contains(&ip_str) {
                                hosts.push(ip_str);
                            }
                        }
                    }
                    Err(e) => {
                        warn!("Error resolving IP for {:?}: {}", addr, e);
                    }
                }
            }
            for host in &hosts {
                if let Some(data) = self.data_map.get(&rule.name) {
                    if match_exclude(host, &rule.exclude) {
                        continue;
                    }
                    if data.match_host(host) {
                        return (rule.name.clone(), rule.select.clone());
                    }
                } else {
                    warn!("Error: Route data '{}' not found in dataMap", rule.name);
                }
            }
        }
        ("default".to_string(), self.default_tag.clone())
    }

    // pub(crate) fn route_old(&self, addr: &RunAddr) -> String {
    //     for rule in &self.rules {
    //         if let Some(data) = self.data_map.get(&rule.name) {
    //             if !match_exclude(addr.addr.as_str(), &rule.exclude) {
    //                 if match_route_data(addr.addr.as_str(), &data.lines, &data.format).is_some() {
    //                     return rule.select.clone();
    //                 }
    //             }
    //         } else {
    //             warn!("Error: Route data '{}' not found in dataMap", rule.name);
    //         }
    //     }
    //     self.default_tag.clone()
    // }
}
