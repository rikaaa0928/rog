use std::env;
use futures::future::select_all;
use log::error;
use tokio::{fs, select, spawn};
use crate::def::config::Config;
use crate::object::config::ObjectConfig;
use crate::object::Object;

mod connector;
mod def;
mod listener;
mod stream;
mod test;
mod util;
mod object;
mod router;

#[tokio::main]
async fn main() -> std::io::Result<()> {
    if env::var("RUST_LOG").is_err() {
        env::set_var("RUST_LOG", "info")
    }
    env_logger::init();

    let contents = fs::read_to_string(env::var("ROG_CONFIG").unwrap()).await?;

    // 解析 TOML
    let cfg_res = toml::from_str::<Config>(&contents);
    if cfg_res.is_err() {
        return Err(std::io::Error::new(std::io::ErrorKind::Other, "invalid config file"));
    }
    let cfg = cfg_res.unwrap();
    let mut fs = Vec::new();
    for l in cfg.clone().listener {
        let cfg = cfg.clone();
        fs.push(spawn(async move {
            let obj_conf = ObjectConfig::build(l.name.as_str(), &(cfg.clone()));
            let obj = Object::new(obj_conf);
            obj.start().await
        }));
    }
    let (a, _, _) = select_all(fs).await;
    error!("error: {:?}", a);
    Ok(())
}
