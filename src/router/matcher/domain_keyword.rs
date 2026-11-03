use crate::router::matcher::Matcher;
use aho_corasick::{AhoCorasick, MatchKind};
use std::net::IpAddr;

pub struct DomainKeywordMatcher {
    ac: Option<AhoCorasick>,
}

impl Matcher for DomainKeywordMatcher {
    fn match_host(&self, host: &str) -> bool {
        if host.trim_matches(['[', ']']).parse::<IpAddr>().is_ok() {
            return false;
        }
        if let Some(ref ac) = self.ac {
            let trimmed = host.trim();
            let lower: std::borrow::Cow<'_, str> =
                if trimmed.bytes().any(|b| b.is_ascii_uppercase()) {
                    std::borrow::Cow::Owned(trimmed.to_ascii_lowercase())
                } else {
                    std::borrow::Cow::Borrowed(trimmed)
                };
            ac.find(lower.as_ref()).is_some()
        } else {
            false
        }
    }
}

pub(crate) fn domain_keyword_matcher_factory(
    lines: Vec<String>,
    _data: Vec<u8>,
) -> Box<dyn Matcher> {
    let mut keywords = Vec::with_capacity(lines.len());
    for line in lines {
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }
        keywords.push(trimmed.to_ascii_lowercase());
    }
    keywords.shrink_to_fit();
    let ac = if keywords.is_empty() {
        None
    } else {
        AhoCorasick::builder()
            .match_kind(MatchKind::Standard)
            .build(&keywords)
            .ok()
    };
    Box::new(DomainKeywordMatcher { ac })
}
