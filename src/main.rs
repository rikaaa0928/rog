use crate::def::config::Config;
use crate::object::config::ObjectConfig;
use crate::object::Object;
use futures::future::select_all;
use log::error;
use std::env;
use std::sync::Arc;
use tokio::{fs, spawn};

#[global_allocator]
static GLOBAL: mimalloc::MiMalloc = mimalloc::MiMalloc;


mod connector;
mod def;
mod listener;
mod object;
mod proto;
mod router;
mod stream;
mod test;
mod util;

#[tokio::main]
async fn main() -> std::io::Result<()> {
    if env::var("RUST_LOG").is_err() {
        env::set_var("RUST_LOG", "info");
    }
    env_logger::init();
    if env::var("ROG_CONFIG").is_err() {
        env::set_var("ROG_CONFIG", "/etc/rog/config.toml");
    }

    let contents = fs::read_to_string(env::var("ROG_CONFIG").unwrap()).await?;

    // 解析 TOML
    let cfg_res = toml::from_str::<Config>(&contents);
    if cfg_res.is_err() {
        return Err(std::io::Error::other(
            String::from("invalid config file: ") + cfg_res.err().unwrap().message(),
        ));
    }
    let cfg = cfg_res.unwrap();
    let resolver = router::resolver::Resolver::new();
    let router = router::DefaultRouter::new(
        cfg.router.as_slice(),
        cfg.data.as_ref().unwrap_or(&vec![]).as_slice(),
        resolver,
    )
    .await;
    let router = Arc::new(router);
    let mut fs = Vec::new();
    for l in cfg.clone().listener {
        let cfg = cfg.clone();
        let router = router.clone();
        fs.push(spawn(async move {
            let obj_conf = Arc::new(ObjectConfig::build(l.name.as_str(), &cfg));
            let obj = Object::new(obj_conf, router.clone());
            obj.start().await
        }));
    }
    let (a, _, _) = select_all(fs).await;
    error!("error: {:?}", a);
    Ok(())
}
