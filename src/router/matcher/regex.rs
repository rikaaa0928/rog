use std::net::IpAddr;
use regex::Regex;
use ipnetwork::IpNetwork;

use crate::router::matcher::Matcher;


pub(crate) fn regex_matcher_factory(lines: Vec<String>, _data: Vec<u8>) -> Box<dyn Matcher> {
    let mut inner: Vec<RegexLine> = Vec::with_capacity(lines.len());
    for l in lines {
        let trimmed_line = l.trim();
        if trimmed_line.is_empty() {
            continue;
        }
        inner.push(parse_regex_line(trimmed_line));
    }
    Box::new(RegexMatcher { lines: inner })
}

pub struct RegexMatcher {
    lines: Vec<RegexLine>,
}

impl Matcher for RegexMatcher {
    fn match_host(&self, host: &str) -> bool {
        for item in &self.lines {
            if item.match_line(host) {
                return true;
            }
        }
        false
    }
}

struct RegexLine {
    regex: Option<Regex>,
    cidr: Option<IpNetwork>,
    host: Option<String>,
}

fn parse_regex_line(line: &str) -> RegexLine {
    if let Ok(cidr) = line.parse::<IpNetwork>() {
        return RegexLine { cidr: Some(cidr), regex: None, host: None };
    }
    if let Ok(re) = Regex::new(line) {
        return RegexLine { regex: Some(re), cidr: None, host: None };
    }
    RegexLine { host: Some(line.to_owned()), cidr: None, regex: None }
}

impl RegexLine {
    fn match_line(&self, host: &str) -> bool {
        if let Ok(ip) = host.parse::<IpAddr>() {
            if self.cidr.is_some() {
                return self.cidr.as_ref().unwrap().contains(ip);
            }
            return self.host.as_ref().map(|h| h == host).unwrap_or(false);
        }
        if self.regex.is_some() {
            return self.regex.as_ref().unwrap().is_match(host);
        }
        self.host.as_ref().map(|h| h == host).unwrap_or(false)
    }
}
