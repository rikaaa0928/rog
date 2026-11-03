use crate::router::matcher::Matcher;
use ipnetwork::IpNetwork;
use std::collections::HashSet;
use std::net::IpAddr;

pub(crate) fn cidr_matcher_factory(lines: Vec<String>, _data: Vec<u8>) -> Box<dyn Matcher> {
    let mut raw_cidrs = Vec::new();
    let mut v4_nets = Vec::new();
    let mut v6_nets = Vec::new();
    let mut hosts = HashSet::new();

    for l in lines {
        let trimmed_line = l.trim();
        if trimmed_line.is_empty() || trimmed_line.starts_with('#') {
            continue;
        }
        if let Ok(cidr) = trimmed_line.parse::<IpNetwork>() {
            raw_cidrs.push(cidr);
            match cidr {
                IpNetwork::V4(v4) => {
                    let mask = u32::from(v4.mask());
                    let net = u32::from(v4.ip()) & mask;
                    v4_nets.push((net, mask));
                }
                IpNetwork::V6(v6) => {
                    let mask = u128::from(v6.mask());
                    let net = u128::from(v6.ip()) & mask;
                    v6_nets.push((net, mask));
                }
            }
        } else {
            hosts.insert(trimmed_line.trim_matches(['[', ']']).to_string());
        }
    }

    Box::new(CIDRMatcher {
        raw_cidrs,
        v4_nets,
        v6_nets,
        hosts,
    })
}

pub struct CIDRMatcher {
    raw_cidrs: Vec<IpNetwork>,
    v4_nets: Vec<(u32, u32)>,
    v6_nets: Vec<(u128, u128)>,
    hosts: HashSet<String>,
}

impl Matcher for CIDRMatcher {
    fn cidrs(&self) -> Vec<IpNetwork> {
        self.raw_cidrs.clone()
    }

    fn match_host(&self, host: &str) -> bool {
        let trimmed = host.trim_matches(['[', ']']);
        match trimmed.parse::<IpAddr>() {
            Ok(IpAddr::V4(v4)) => {
                let ip_bits = u32::from(v4);
                for &(net, mask) in &self.v4_nets {
                    if (ip_bits & mask) == net {
                        return true;
                    }
                }
                !self.hosts.is_empty() && self.hosts.contains(trimmed)
            }
            Ok(IpAddr::V6(v6)) => {
                let ip_bits = u128::from(v6);
                for &(net, mask) in &self.v6_nets {
                    if (ip_bits & mask) == net {
                        return true;
                    }
                }
                !self.hosts.is_empty() && self.hosts.contains(trimmed)
            }
            Err(_) => {
                !self.hosts.is_empty()
                    && (self.hosts.contains(trimmed) || self.hosts.contains(host))
            }
        }
    }
}
