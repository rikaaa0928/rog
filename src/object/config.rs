use std::collections::{HashMap, HashSet};
use serde::Deserialize;
use crate::def::config;

#[derive(Deserialize, Debug, Clone)]
pub struct ObjectConfig {
    pub listener: config::Listener,
    pub router: config::Router,
    pub connector: HashMap<String, config::Connector>,
}

impl ObjectConfig {
    pub fn build(name: &str, cfg: &config::Config) -> Self {
        let mut this_cfg: Option<config::Listener> = None;
        for conf in &cfg.listener {
            if name == conf.name {
                this_cfg = Some(conf.clone());
                break;
            }
        }
        let listener = this_cfg.unwrap();
        let binding = (&listener).router.to_owned();
        let router_name = binding.as_str();
        let mut this_router: Option<config::Router> = None;
        let mut c_names: HashSet<String> = HashSet::new();
        c_names.insert("default".to_string());
        for conf in &cfg.router {
            if router_name == conf.name {
                c_names.insert(conf.default.to_string());
                this_router = Some(conf.clone());
            }
        }
        if this_router.is_none() {
            this_router = Some(config::Router { name: "default".to_string(), default: "default".to_string() })
        }
        let mut connector = HashMap::new();
        for c_name in c_names {
            for conn in &cfg.connector {
                if c_name == conn.name {
                    connector.insert(conn.name.to_string(), conn.clone());
                }
            }
        }

        Self {
            listener,
            router: this_router.unwrap(),
            connector,
        }
    }
}