use serde::Deserialize;
use std::collections::HashMap;

#[derive(Deserialize, Debug, Clone)]
pub struct Config {
    pub reverse_server: Option<ReverseServer>,
    pub listener: Vec<Listener>,
    pub router: Vec<Router>,
    pub data: Option<Vec<RouteData>>,
    pub connector: Vec<Connector>,
    pub server_id: Option<String>,
    pub buffer_size: Option<String>,
}

#[derive(Deserialize, Debug, Clone)]
pub struct ReverseServer {
    pub endpoint: String,
    pub options: Option<HashMap<String, toml::Value>>,
}

#[derive(Deserialize, Debug, Clone)]
pub struct Connector {
    pub endpoint: Option<String>,
    pub name: String,
    pub user: Option<String>,
    pub pw: Option<String>,
    pub proto: String,
    pub options: Option<HashMap<String, toml::Value>>,
}

#[derive(Deserialize, Debug, Clone)]
pub struct Router {
    pub name: String,
    pub default: String,
    pub route_rules: Option<Vec<RouteRule>>,
}

#[derive(Deserialize, Debug, Clone)]
pub struct Listener {
    pub endpoint: String,
    pub name: String,
    pub user: Option<String>,
    pub pw: Option<String>,
    pub proto: String,
    pub router: String,
    pub options: Option<HashMap<String, toml::Value>>,
}

#[derive(Deserialize, Debug, Clone)]
pub struct RouteRule {
    pub name: String,
    pub select: String,
    pub exclude: Vec<String>,
    pub domain_to_ip: Option<bool>,
    pub dns: Option<String>,
}

pub fn get_option_bool(options: &Option<HashMap<String, toml::Value>>, key: &str) -> bool {
    options
        .as_ref()
        .and_then(|m| m.get(key))
        .and_then(|v| v.as_bool())
        .unwrap_or(false)
}

#[derive(Deserialize, Debug, Clone)]
pub struct RouteData {
    pub name: String,
    pub url: Option<String>,
    pub format: String,
    pub data: Option<String>,
}
