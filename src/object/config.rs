use std::collections::HashSet;
use crate::def;
use crate::def::config;

pub struct ObjectConfig {
    pub listener: config::Listener,
    pub router: Option<config::Router>,
    pub connector: Vec<config::Connector>,
}

impl ObjectConfig {
    fn build(name: &str, cfg: &config::Config) -> Self {
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
        let mut connector = Vec::new();
        for c_name in c_names {
            for conn in &cfg.connector {
                if c_name == conn.name {
                    connector.push(conn.clone());
                }
            }
        }

        Self {
            listener,
            router: this_router,
            connector,
        }
    }
}