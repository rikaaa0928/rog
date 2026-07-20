use crate::block::BlockManager;
use crate::consts::TCP_IO_BUFFER_SIZE;
use crate::def::config::Config;
use crate::object::Object;
use crate::object::config::ObjectConfig;
use futures::future::select_all;
use log::error;
use proxy_observe::ObserveRegistry;
use std::collections::HashMap;
use std::env;
use std::path::Path;
use std::sync::Arc;
use tokio::{fs, spawn};
use tokio_util::sync::CancellationToken;

pub mod block;
mod connector;
mod consts;
pub mod def;
mod listener;
mod object;
mod proto;
mod router;
mod stream;
mod test;
#[cfg(test)]
mod test_tcp_buffer;
mod util;

#[derive(Clone)]
pub struct RunOptions {
    pub config: Config,
    pub observe_registry: ObserveRegistry,
    pub shutdown: CancellationToken,
}

pub fn load_config_str(contents: &str) -> std::io::Result<Config> {
    toml::from_str::<Config>(contents)
        .map_err(|e| std::io::Error::other(format!("invalid config file: {}", e.message())))
}

pub fn load_config_file(path: impl AsRef<Path>) -> std::io::Result<Config> {
    let contents = std::fs::read_to_string(path)?;
    load_config_str(&contents)
}

pub async fn run(options: RunOptions) -> std::io::Result<()> {
    let cfg = options.config;
    let observe_registry = options.observe_registry;
    let shutdown = options.shutdown;

    let buffer_size = if let Some(n) = &cfg.buffer_size {
        util::parse::parse_size(n)?
    } else {
        0
    };

    let block_manager = if buffer_size > 0 {
        let block_number = buffer_size.div_ceil(TCP_IO_BUFFER_SIZE as u64);
        log::info!(
            "Global buffer pool size: {} bytes with {} block",
            buffer_size,
            block_number
        );
        Some(Arc::new(BlockManager::new(block_number)))
    } else {
        None
    };

    let resolver = router::resolver::Resolver::new();
    let router = router::DefaultRouter::new(
        cfg.router.as_slice(),
        cfg.data.as_ref().unwrap_or(&vec![]).as_slice(),
        resolver,
    )
    .await;
    let router = Arc::new(router);

    spawn(observe_registry.sampler_task());
    if let Some(listen_addr) = proxy_observe::env_listen_addr() {
        spawn(proxy_observe::api::serve(
            observe_registry.clone(),
            listen_addr,
        ));
    }

    if let Some(rev_server) = cfg.reverse_server.clone() {
        let pw_map: HashMap<String, Option<String>> = cfg
            .connector
            .iter()
            .filter(|c| c.proto == "rev_grpc")
            .map(|c| (c.name.clone(), c.pw.clone()))
            .collect();
        crate::connector::rev_grpc::start_reverse_server(
            rev_server.endpoint,
            pw_map,
            &rev_server.options,
        )
        .await;
    }

    let mut tasks = Vec::new();
    let server_id = cfg
        .server_id
        .clone()
        .unwrap_or(uuid::Uuid::new_v4().to_string());
    for listener in cfg.clone().listener {
        let cfg = cfg.clone();
        let router = router.clone();
        let server_id = server_id.clone();
        let block_manager = block_manager.clone();
        let observe_registry = observe_registry.clone();
        let shutdown = shutdown.clone();
        tasks.push(spawn(async move {
            let obj_conf = Arc::new(ObjectConfig::build(listener.name.as_str(), &cfg, server_id));
            let obj = Object::new(obj_conf, router, block_manager, observe_registry);
            obj.start_with_shutdown(shutdown).await
        }));
    }

    if tasks.is_empty() {
        shutdown.cancelled().await;
    } else {
        tokio::select! {
            _ = shutdown.cancelled() => {}
            (result, _, _) = select_all(tasks) => {
                error!("error: {:?}", result);
            }
        }
    }

    Ok(())
}

pub async fn run_config_file(path: impl AsRef<Path>) -> std::io::Result<()> {
    let contents = fs::read_to_string(path).await?;
    let cfg = load_config_str(&contents)?;
    let observe_registry = ObserveRegistry::new();
    run(RunOptions {
        config: cfg,
        observe_registry,
        shutdown: CancellationToken::new(),
    })
    .await
}

pub async fn run_from_env() -> std::io::Result<()> {
    if env::args()
        .skip(1)
        .any(|arg| arg == "--version" || arg == "-V")
    {
        println!("rog {}", env!("CARGO_PKG_VERSION"));
        return Ok(());
    }

    if env::var("RUST_LOG").is_err() {
        unsafe {
            env::set_var("RUST_LOG", "info");
        }
    }
    env_logger::init();
    if env::var("ROG_CONFIG").is_err() {
        unsafe {
            env::set_var("ROG_CONFIG", "/etc/rog/config.toml");
        }
    }

    run_config_file(env::var("ROG_CONFIG").unwrap()).await
}
