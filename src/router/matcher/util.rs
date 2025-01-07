use crate::router::consts::FORMAT_REGEX;

pub fn match_ip_or_cidr(host: &str, rule: &str) -> bool {
    if rule.contains('/') {
        if let Ok(network) = rule.parse::<ipnet::IpNet>() {
            if let Ok(ip) = host.parse::<std::net::IpAddr>() {
                return network.contains(&ip);
            }
        }
    } else if let Ok(_ip) = rule.parse::<std::net::IpAddr>() {
        return host == rule;
    }
    false
}

pub fn match_exclude(host: &str, excludes: &[String]) -> bool {
    for exclude in excludes {
        if match_ip_or_cidr(host, exclude) {
            return true;
        } else if let Ok(re) = regex::Regex::new(exclude) {
            if re.is_match(host) {
                return true;
            }
        }
    }
    false
}

pub fn match_route_data(host: &str, data: &[String], format: &str) -> Option<String> {
    for item in data {
        if match_ip_or_cidr(host, item) {
            return Some(item.clone());
        }
        if format == FORMAT_REGEX {
            if let Ok(re) = regex::Regex::new(item) {
                if re.is_match(host) {
                    return Some(item.clone());
                }
            }
        } else if item == host {
            return Some(item.clone());
        }
    }
    None
}