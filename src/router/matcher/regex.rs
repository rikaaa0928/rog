use ipnetwork::IpNetwork;
use regex::RegexSet;
use std::collections::HashSet;
use std::net::IpAddr;

use crate::router::matcher::Matcher;

pub(crate) fn regex_matcher_factory(lines: Vec<String>, _data: Vec<u8>) -> Box<dyn Matcher> {
    let mut v4_nets = Vec::new();
    let mut v6_nets = Vec::new();
    let mut patterns = Vec::new();
    let mut hosts = HashSet::new();

    for l in lines {
        let trimmed_line = l.trim();
        if trimmed_line.is_empty() {
            continue;
        }
        if let Ok(cidr) = trimmed_line.parse::<IpNetwork>() {
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
        } else if regex::Regex::new(trimmed_line).is_ok() {
            patterns.push(trimmed_line.to_owned());
        } else {
            hosts.insert(trimmed_line.trim_matches(['[', ']']).to_owned());
        }
    }

    let regex_set = if patterns.is_empty() {
        None
    } else {
        RegexSet::new(&patterns).ok()
    };

    Box::new(RegexMatcher {
        regex_set,
        v4_nets,
        v6_nets,
        hosts,
    })
}

pub struct RegexMatcher {
    regex_set: Option<RegexSet>,
    v4_nets: Vec<(u32, u32)>,
    v6_nets: Vec<(u128, u128)>,
    hosts: HashSet<String>,
}

impl Matcher for RegexMatcher {
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
                !self.hosts.is_empty()
                    && (self.hosts.contains(trimmed) || self.hosts.contains(host))
            }
            Ok(IpAddr::V6(v6)) => {
                let ip_bits = u128::from(v6);
                for &(net, mask) in &self.v6_nets {
                    if (ip_bits & mask) == net {
                        return true;
                    }
                }
                !self.hosts.is_empty()
                    && (self.hosts.contains(trimmed) || self.hosts.contains(host))
            }
            Err(_) => {
                if let Some(ref set) = self.regex_set {
                    if set.is_match(host) {
                        return true;
                    }
                }
                !self.hosts.is_empty()
                    && (self.hosts.contains(trimmed) || self.hosts.contains(host))
            }
        }
    }
}
