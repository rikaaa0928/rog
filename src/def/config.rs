use serde::Deserialize;

#[derive(Deserialize, Debug, Clone)]
pub struct Config {
    pub listener: Vec<Listener>,
    pub router: Vec<Router>,
    pub data: Vec<RouteData>,
    pub connector: Vec<Connector>,
}

#[derive(Deserialize, Debug, Clone)]
pub struct Connector {
    pub endpoint: Option<String>,
    pub name: String,
    pub user: Option<String>,
    pub pw: Option<String>,
    pub proto: String,
}

#[derive(Deserialize, Debug, Clone)]
pub struct Router {
    pub name: String,
    pub default: String,
    pub route_rules: Vec<RouteRule>,
}

#[derive(Deserialize, Debug, Clone)]
pub struct Listener {
    pub endpoint: String,
    pub name: String,
    pub user: Option<String>,
    pub pw: Option<String>,
    pub proto: String,
    pub router: String,
}

#[derive(Deserialize, Debug, Clone)]
pub struct RouteRule {
    pub name: String,
    pub select: String,
    pub exclude: Vec<String>,
    pub domain_to_ip: Option<bool>,
}

#[derive(Deserialize, Debug, Clone)]
pub struct RouteData {
    pub name: String,
    pub url: Option<String>,
    pub format: String,
    pub data: Option<String>,
}
