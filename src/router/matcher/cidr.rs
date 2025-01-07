use std::net::IpAddr;
use ipnetwork::IpNetwork;
use crate::router::matcher::{ Matcher};

pub(crate) fn cidr_matcher_factory(lines: Vec<String>, _data: Vec<u8>) -> Box<dyn Matcher> {
    let mut inner: Vec<CidrLine> = Vec::with_capacity(lines.len());
    for l in lines {
        let trimmed_line = l.trim();
        if trimmed_line.is_empty() {
            continue;
        }
        if let Ok(cidr) = trimmed_line.parse::<IpNetwork>() {
            inner.push(CidrLine { cidr: Some(cidr), host: None });
        } else {
            inner.push(CidrLine { cidr: None, host: Some(trimmed_line.to_string()) });
        }
    }
    Box::new(CIDRMatcher { lines: inner })
}

pub struct CIDRMatcher {
    lines: Vec<CidrLine>,
}

impl Matcher for CIDRMatcher {
    fn match_host(&self, host: &str) -> bool {
        for item in &self.lines {
            if item.match_line(host) {
                return true;
            }
        }
        false
    }
}

struct CidrLine {
    cidr: Option<IpNetwork>,
    host: Option<String>,
}

impl CidrLine {
    fn match_line(&self, line: &str) -> bool {
        if let Some(cidr) = &self.cidr {
            if let Ok(ip) = line.parse::<IpAddr>() {
                return cidr.contains(ip);
            }
        }
        self.host.as_ref().map(|h| h == line).unwrap_or(false)
    }
}
