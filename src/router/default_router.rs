use crate::def::config::RouteRule;
use crate::router::matcher::Matcher;
use crate::router::matcher::util::ExcludeMatcher;
use crate::router::resolver::Resolver;
use crate::util::RunAddr;
use log::warn;
use std::collections::HashMap;
use std::sync::Arc;

pub struct DefaultBaseRouter {
    name: String,
    default_tag: String,
    rules: Vec<CompiledRouteRule>,
    data_map: Arc<HashMap<String, Box<dyn Matcher>>>,
    resolver: Arc<Resolver>,
}

struct CompiledRouteRule {
    name: String,
    select: String,
    domain_to_ip: bool,
    dns: Option<String>,
    exclude: ExcludeMatcher,
}

impl CompiledRouteRule {
    fn new(rule: RouteRule) -> Self {
        Self {
            name: rule.name,
            select: rule.select,
            domain_to_ip: rule.domain_to_ip.unwrap_or(false),
            dns: rule.dns,
            exclude: ExcludeMatcher::new(&rule.exclude),
        }
    }
}

impl DefaultBaseRouter {
    pub fn new(
        name: String,
        default_tag: String,
        rules: Vec<RouteRule>,
        data_map: Arc<HashMap<String, Box<dyn Matcher>>>,
        resolver: Arc<Resolver>,
    ) -> Self {
        let rules = rules.into_iter().map(CompiledRouteRule::new).collect();
        DefaultBaseRouter {
            name,
            default_tag,
            rules,
            data_map,
            resolver,
        }
    }

    pub(crate) fn route_dns(&self, domain: &str) -> crate::def::DnsRouteDecision {
        for rule in &self.rules {
            if let Some(data) = self.data_map.get(&rule.name) {
                if data.kind() != crate::router::matcher::MatchKind::Ip
                    && !rule.exclude.is_match(domain)
                    && data.match_host(domain)
                {
                    return crate::def::DnsRouteDecision {
                        outbound: rule.select.clone(),
                        matched_rule: Some(rule.name.clone()),
                    };
                }
            }
        }
        crate::def::DnsRouteDecision {
            outbound: self.default_tag.clone(),
            matched_rule: None,
        }
    }
    pub(crate) fn route_mapped(&self, ip: &str, domains: &[String]) -> String {
        use crate::router::matcher::MatchKind;
        for rule in &self.rules {
            let Some(data) = self.data_map.get(&rule.name) else {
                continue;
            };
            let mut candidates = vec![];
            if data.kind() != MatchKind::Domain {
                candidates.push(ip);
            }
            if data.kind() != MatchKind::Ip {
                candidates.extend(domains.iter().map(String::as_str));
            }
            if candidates
                .into_iter()
                .any(|host| !rule.exclude.is_match(host) && data.match_host(host))
            {
                return rule.select.clone();
            }
        }
        self.default_tag.clone()
    }

    pub(crate) async fn route(&self, addr: &RunAddr) -> (String, String) {
        for rule in &self.rules {
            let mut hosts = vec![addr.addr.clone().to_string()];
            if rule.domain_to_ip {
                let dns = rule.dns.as_deref().unwrap_or("");
                match self.resolver.resolve_ip(addr.addr.as_str(), dns).await {
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
                    if rule.exclude.is_match(host) {
                        continue;
                    }
                    if data.match_host(host) {
                        return (rule.name.clone(), rule.select.clone());
                    }
                } else {
                    warn!(
                        "Error: Route data '{}' not found in router '{}' dataMap",
                        rule.name, self.name
                    );
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

#[cfg(test)]
mod dns_tests {
    use super::*;
    use crate::router::matcher::{TypedMatcher, get_matcher_factory_fn, match_kind};
    fn build(order: &[(&str, &str, &str, Vec<String>)]) -> DefaultBaseRouter {
        let mut data = HashMap::new();
        let mut rules = vec![];
        for (i, (format, value, out, exclude)) in order.iter().enumerate() {
            let name = i.to_string();
            data.insert(
                name.clone(),
                Box::new(TypedMatcher {
                    inner: get_matcher_factory_fn(format).unwrap()(vec![value.to_string()], vec![]),
                    kind: match_kind(format),
                }) as Box<dyn Matcher>,
            );
            rules.push(RouteRule {
                name,
                select: out.to_string(),
                exclude: exclude.clone(),
                domain_to_ip: Some(true),
                dns: Some("invalid".into()),
            });
        }
        DefaultBaseRouter::new(
            "main".into(),
            "default".into(),
            rules,
            Arc::new(data),
            Resolver::new(),
        )
    }
    #[tokio::test]
    async fn rule_order_candidates_and_dns_projection() {
        let domains = vec!["b.example".into(), "c.example".into()];
        let r = build(&[
            ("domain-suffix", "c.example", "C", vec![]),
            ("cidr", "1.1.1.1/32", "IP", vec![]),
            ("domain-suffix", "b.example", "B", vec![]),
        ]);
        assert_eq!(r.route_mapped("1.1.1.1", &domains), "C");
        assert_eq!(r.route_dns("c.example").outbound, "C");
        assert!(r.route_dns("unmatched").matched_rule.is_none());
        let r = build(&[
            ("cidr", "1.1.1.1/32", "IP", vec![]),
            (
                "domain-suffix",
                "example",
                "D",
                vec!["^b\\.example$".into()],
            ),
        ]);
        assert_eq!(r.route_mapped("1.1.1.1", &domains), "IP");
        assert_eq!(r.route_dns("c.example").outbound, "D");
        assert_eq!(r.route_mapped("8.8.8.8", &domains), "D");
    }
}
