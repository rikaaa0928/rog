use crate::def;
use crate::def::config;
use crate::util::RunAddr;

pub struct DefaultRouter {
    inner: config::Router,
}

impl DefaultRouter {
    pub fn new(cfg: &config::Router) -> Self {
        Self {
            inner: cfg.clone(),
        }
    }
}

#[async_trait::async_trait]
impl def::Router for DefaultRouter {
    async fn route(&self, _: &RunAddr) -> std::io::Result<String> {
        Ok(self.inner.default.clone())
    }
}