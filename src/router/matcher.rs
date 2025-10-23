mod cidr;
mod regex;
mod test;
pub mod util;

use crate::router::consts;
use crate::router::matcher::cidr::cidr_matcher_factory;
use crate::router::matcher::regex::regex_matcher_factory;

pub trait Matcher: Send + Sync {
    fn match_host(&self, host: &str) -> bool;
}

pub type MatcherFactoryFn = fn(lines: Vec<String>, data: Vec<u8>) -> Box<dyn Matcher>;

pub fn get_matcher_factory_fn(name: &str) -> Option<MatcherFactoryFn> {
    match name {
        consts::FORMAT_CIDR => Some(cidr_matcher_factory),
        consts::FORMAT_REGEX=> Some(regex_matcher_factory),
        _ => None,
    }
}
