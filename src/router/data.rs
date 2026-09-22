use crate::def::config;
use crate::router::consts;
use crate::router::consts::FORMAT_LAN;
use crate::router::matcher::{Matcher, get_matcher_factory_fn};
use crate::router::remote_matcher::DynamicRemoteMatcher;
use serde::Deserialize;
use std::collections::HashMap;
use std::time::Duration;

#[derive(Deserialize, Debug, Clone)]
pub struct InnerRouteData {
    pub name: String,
    pub data: Vec<u8>,
    pub lines: Vec<String>,
    pub format: String,
}

pub async fn load_route_data(data_cfg: &[config::RouteData]) -> HashMap<String, Box<dyn Matcher>> {
    let mut data_map: HashMap<String, Box<dyn Matcher>> = HashMap::new();
    for rd_cfg in data_cfg {
        let rd_cfg = &rd_cfg;
        let mut rd = InnerRouteData {
            name: rd_cfg.name.clone(),
            data: Vec::new(),
            lines: Vec::new(),
            format: rd_cfg.format.clone(),
        };

        if rd.format == FORMAT_LAN {
            rd.lines = vec![
                "10.0.0.0/8".to_string(),
                "172.16.0.0/12".to_string(),
                "192.168.0.0/16".to_string(),
                "127.0.0.0/8".to_string(),
            ];
            rd.format = consts::FORMAT_CIDR.to_string();
        } else if let Some(ref url) = rd_cfg.url {
            let interval = Duration::from_secs(rd_cfg.interval.unwrap_or(3600));
            let matcher = DynamicRemoteMatcher::new(
                rd_cfg.name.clone(),
                url.clone(),
                rd_cfg.format.clone(),
                interval,
            );
            data_map.insert(rd_cfg.name.clone(), Box::new(matcher));
            continue;
        } else if rd_cfg.data.is_some() {
            rd.lines = rd_cfg
                .data
                .as_ref()
                .unwrap()
                .split("\n")
                .filter(|s| !s.trim().is_empty())
                .map(|s| s.to_string())
                .collect();
        } else {
            log::warn!("No data source provided for route data '{}'", rd_cfg.name);
            continue;
        }
        if let Some(factory) = get_matcher_factory_fn(&rd.format) {
            data_map.insert(rd_cfg.name.clone(), factory(rd.lines, rd.data));
        }
    }
    data_map
}
