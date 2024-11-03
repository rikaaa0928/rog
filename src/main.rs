use std::env;
use tokio::fs;
use crate::def::config::Config;

mod connector;
mod def;
mod listener;
mod stream;
mod test;
mod util;
mod object;

#[tokio::main]
async fn main() -> std::io::Result<()> {
    if env::var("RUST_LOG").is_err() {
        env::set_var("RUST_LOG", "info")
    }
    env_logger::init();

    let contents = fs::read_to_string("config/config.toml").await?;

    // 解析 TOML
    let cfg_res = toml::from_str::<Config>(&contents);
    if cfg_res.is_err() {
        return Err(std::io::Error::new(std::io::ErrorKind::Other, "invalid config file"));
    }
    let cfg = cfg_res.unwrap();
    Ok(())
}
