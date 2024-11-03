use serde::Deserialize;

#[derive(Deserialize, Debug, Clone)]
pub struct Config {
    pub listener: Vec<Listener>,
    pub router: Vec<Router>,
    pub connector: Vec<Connector>,
}

#[derive(Deserialize, Debug, Clone)]
pub struct Connector {
    pub endpoint: String,
    pub name: String,
    pub user: Option<String>,
    pub pw: Option<String>,
    pub proto: String,
}

#[derive(Deserialize, Debug, Clone)]
pub struct Router {
    pub name: String,
    pub default: String,
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