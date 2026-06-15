use regex::Regex;
use std::net::IpAddr;

pub struct ExcludeMatcher {
    rules: Vec<ExcludeRule>,
}

enum ExcludeRule {
    Ip(IpAddr),
    Cidr(ipnet::IpNet),
    Regex(Regex),
}

impl ExcludeMatcher {
    pub fn new(excludes: &[String]) -> Self {
        let mut rules = Vec::with_capacity(excludes.len());
        for exclude in excludes {
            if exclude.contains('/') {
                if let Ok(network) = exclude.parse::<ipnet::IpNet>() {
                    rules.push(ExcludeRule::Cidr(network));
                }
            } else if let Ok(ip) = exclude.parse::<IpAddr>() {
                rules.push(ExcludeRule::Ip(ip));
            } else if let Ok(re) = Regex::new(exclude) {
                rules.push(ExcludeRule::Regex(re));
            }
        }
        Self { rules }
    }

    pub fn is_match(&self, host: &str) -> bool {
        let ip = host.parse::<IpAddr>().ok();
        for rule in &self.rules {
            match rule {
                ExcludeRule::Ip(rule_ip) => {
                    if ip.as_ref() == Some(rule_ip) {
                        return true;
                    }
                }
                ExcludeRule::Cidr(network) => {
                    if let Some(ip) = ip {
                        if network.contains(&ip) {
                            return true;
                        }
                    }
                }
                ExcludeRule::Regex(re) => {
                    if re.is_match(host) {
                        return true;
                    }
                }
            }
        }
        false
    }
}
