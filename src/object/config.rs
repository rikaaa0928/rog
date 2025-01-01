use crate::def::{config, RouterSet};
use serde::Deserialize;
use std::collections::{HashMap, HashSet};
use std::sync::Arc;

#[derive(Deserialize, Debug, Clone)]
pub struct ObjectConfig {
    pub listener: config::Listener,
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

        let mut connector = HashMap::new();

        for conn in &cfg.connector {
            connector.insert(conn.name.to_string(), conn.clone());
        }

        Self {
            listener,
            connector,
        }
    }
}
