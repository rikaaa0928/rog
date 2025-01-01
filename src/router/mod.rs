mod consts;
mod data;
mod router;
mod util;

use crate::def;
use crate::def::config::{RouteData, Router};
use crate::router::data::{load_route_data, InnerRouteData};
use crate::router::router::DefaultBaseRouter;
use crate::util::RunAddr;
use serde::Deserialize;
use std::collections::HashMap;
use std::sync::Arc;

#[derive(Debug, Clone)]
pub struct DefaultRouter {
    router_map: HashMap<String, DefaultBaseRouter>,
    data_map: Arc<HashMap<String, InnerRouteData>>,
}

#[async_trait::async_trait]
impl def::RouterSet for DefaultRouter {
    async fn route(&self, name: &str, addr: &RunAddr) -> String {
        if let Some(router) = self.router_map.get(name) {
            let res = router.route(addr);
            log::info!("Route {} {} -> {}", name, addr.addr, res);
            res
        } else {
            "default".to_string()
        }
    }
}

impl DefaultRouter {
    pub async fn new(cfg: &[Router], data_cfg: &[RouteData]) -> Self {
        let data_map = Arc::new(load_route_data(data_cfg).await);
        let mut router_set = DefaultRouter {
            router_map: HashMap::new(),
            data_map: data_map.clone(),
        };
        for r in cfg {
            let router = DefaultBaseRouter {
                name: r.name.clone(),
                default_tag: r.default.clone(),
                rules: r.route_rules.clone(),
                data_map: data_map.clone(),
            };
            router_set.router_map.insert(r.name.clone(), router);
        }
        router_set
    }
}
