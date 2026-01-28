use crate::def::config;
use serde::Deserialize;
use std::collections::HashMap;

#[derive(Deserialize, Debug, Clone)]
pub struct ObjectConfig {
    pub listener: config::Listener,
    pub connector: HashMap<String, config::Connector>,
    pub server_id: String,
}

impl ObjectConfig {
    pub fn build(name: &str, cfg: &config::Config, server_id: String) -> Self {
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
            server_id,
        }
    }
}
