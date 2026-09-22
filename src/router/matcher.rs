mod cidr;
mod domain_keyword;
mod domain_suffix;
mod regex;
mod test;
pub mod util;

use crate::router::consts;
use crate::router::matcher::cidr::cidr_matcher_factory;
use crate::router::matcher::domain_keyword::domain_keyword_matcher_factory;
use crate::router::matcher::domain_suffix::domain_suffix_matcher_factory;
use crate::router::matcher::regex::regex_matcher_factory;

pub trait Matcher: Send + Sync {
    fn match_host(&self, host: &str) -> bool;
}

pub type MatcherFactoryFn = fn(lines: Vec<String>, data: Vec<u8>) -> Box<dyn Matcher>;

pub fn get_matcher_factory_fn(name: &str) -> Option<MatcherFactoryFn> {
    match name {
        consts::FORMAT_CIDR => Some(cidr_matcher_factory),
        consts::FORMAT_REGEX => Some(regex_matcher_factory),
        consts::FORMAT_DOMAIN_SUFFIX
        | consts::FORMAT_DOMAIN_SUFFIX_ALT1
        | consts::FORMAT_DOMAIN_SUFFIX_ALT2 => Some(domain_suffix_matcher_factory),
        consts::FORMAT_DOMAIN_KEYWORD
        | consts::FORMAT_DOMAIN_KEYWORD_ALT1
        | consts::FORMAT_DOMAIN_KEYWORD_ALT2 => Some(domain_keyword_matcher_factory),
        _ => None,
    }
}
