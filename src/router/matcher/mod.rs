mod cidr;
mod def;
mod regex;
mod test;
pub mod util;

use crate::router::matcher::cidr::cidr_matcher_factory;
use crate::router::matcher::regex::regex_matcher_factory;

pub trait Matcher: Send + Sync {
    fn match_host(&self, host: &str) -> bool;
}

pub type MatcherFactoryFn = fn(lines: Vec<String>, data: Vec<u8>) -> Box<dyn Matcher>;

pub fn get_matcher_factory_fn(name: &str) -> Option<MatcherFactoryFn> {
    match name {
        "cicd" => Some(cidr_matcher_factory),
        "regex" => Some(regex_matcher_factory),
        _ => None,
    }
}
