use std::collections::HashMap;
use std::sync::Arc;
use log::warn;
use serde::Deserialize;
use crate::def::{config};
use crate::router::data::InnerRouteData;
use crate::router::util::{match_exclude, match_route_data};
use crate::util::RunAddr;

#[derive(Debug, Clone)]
pub struct DefaultBaseRouter {
    pub name: String,
    pub default_tag: String,
    pub rules: Vec<config::RouteRule>,
    pub data_map: Arc<HashMap<String, InnerRouteData>>,
}

impl DefaultBaseRouter {
    pub(crate) fn route(&self, addr: &RunAddr) -> String {
        for rule in &self.rules {
            if let Some(data) = self.data_map.get(&rule.name) {
                if !match_exclude(addr.addr.as_str(), &rule.exclude) {
                    if match_route_data(addr.addr.as_str(), &data.lines, &data.format).is_some() {
                        return rule.select.clone();
                    }
                }
            } else {
                warn!("Error: Route data '{}' not found in dataMap", rule.name);
            }
        }
        self.default_tag.clone()
    }
}
