mod consts;
mod data;
mod matcher;
pub(crate) mod resolver;
mod default_router;
mod test;

use crate::def;
use crate::def::config::{RouteData, Router};
use crate::router::data::load_route_data;
use crate::router::matcher::Matcher;
use crate::router::resolver::Resolver;
use crate::router::default_router::DefaultBaseRouter;
use crate::util::RunAddr;
use std::collections::HashMap;
use std::sync::Arc;

// #[derive(Clone)]
pub struct DefaultRouter {
    router_map: HashMap<String, DefaultBaseRouter>,
    data_map: Arc<HashMap<String, Box<dyn Matcher>>>,
}

#[async_trait::async_trait]
impl def::RouterSet for DefaultRouter {
    async fn route(&self, l_name: &str, r_name: &str, addr: &RunAddr) -> String {
        if let Some(router) = self.router_map.get(r_name) {
            let (match_name, res) = router.route(addr).await;
            log::info!(
                "Route listener: {} router: {} addr: {} match: {} -> {}",
                l_name,
                r_name,
                addr.addr,
                match_name,
                res
            );
            res
        } else {
            log::info!(
                "Route {} {} {} -> {}",
                l_name,
                r_name,
                addr.addr,
                "default by default"
            );
            "default".to_string()
        }
    }
}

impl DefaultRouter {
    pub async fn new(cfg: &[Router], data_cfg: &[RouteData], resolver: Arc<Resolver>) -> Self {
        let data_map = Arc::new(load_route_data(data_cfg).await);
        let mut router_set = DefaultRouter {
            router_map: HashMap::new(),
            data_map: data_map.clone(),
        };
        for r in cfg {
            let router = DefaultBaseRouter::new(
                r.name.clone(),
                r.default.clone(),
                r.route_rules.clone().unwrap_or(vec![]),
                data_map.clone(),
                resolver.clone(),
            );
            router_set.router_map.insert(r.name.clone(), router);
        }
        router_set
    }
}
