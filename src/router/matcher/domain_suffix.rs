use crate::router::matcher::Matcher;
use std::collections::HashMap;
use std::net::IpAddr;

#[derive(Default, Debug)]
struct TrieNode {
    children: HashMap<String, TrieNode>,
    is_terminal: bool,
}

#[derive(Default, Debug)]
pub struct DomainSuffixTrie {
    root: TrieNode,
}

impl DomainSuffixTrie {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn insert(&mut self, suffix: &str) {
        let mut trimmed = suffix.trim();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            return;
        }
        if trimmed.starts_with("*.") {
            trimmed = &trimmed[2..];
        } else if trimmed.starts_with('.') {
            trimmed = &trimmed[1..];
        }
        let trimmed = trimmed.trim_end_matches('.');
        if trimmed.is_empty() {
            return;
        }

        let mut curr = &mut self.root;
        for label in trimmed.split('.').rev() {
            if label.is_empty() {
                continue;
            }
            if curr.is_terminal {
                // Already subsumed by a shorter ancestor suffix (e.g. google.com already inserted,
                // so sub.google.com is redundant).
                return;
            }
            let lower = label.to_ascii_lowercase();
            curr = curr.children.entry(lower).or_default();
        }
        curr.is_terminal = true;
        curr.children.clear();
    }

    pub fn matches(&self, domain: &str) -> bool {
        let trimmed = domain.trim().trim_end_matches('.');
        if trimmed.is_empty() {
            return false;
        }
        let lower = trimmed.to_ascii_lowercase();
        let mut curr = &self.root;
        for label in lower.split('.').rev() {
            if label.is_empty() {
                continue;
            }
            match curr.children.get(label) {
                Some(next) => {
                    if next.is_terminal {
                        return true;
                    }
                    curr = next;
                }
                None => return false,
            }
        }
        curr.is_terminal
    }
}

pub struct DomainSuffixMatcher {
    trie: DomainSuffixTrie,
}

impl Matcher for DomainSuffixMatcher {
    fn match_host(&self, host: &str) -> bool {
        if host.parse::<IpAddr>().is_ok() {
            return false;
        }
        self.trie.matches(host)
    }
}

pub(crate) fn domain_suffix_matcher_factory(
    lines: Vec<String>,
    _data: Vec<u8>,
) -> Box<dyn Matcher> {
    let mut trie = DomainSuffixTrie::new();
    for line in lines {
        trie.insert(&line);
    }
    Box::new(DomainSuffixMatcher { trie })
}
