use crate::block::BlockManager;
use crate::consts::TCP_IO_BUFFER_SIZE;
use crate::def::config::Config;
use crate::object::Object;
use crate::object::config::ObjectConfig;
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
pub mod dns;
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
    let mut cfg = toml::from_str::<Config>(contents)
        .map_err(|e| std::io::Error::other(format!("invalid config file: {}", e.message())))?;
    cfg.apply_defaults();
    cfg.validate()?;
    Ok(cfg)
}

pub fn load_config_file(path: impl AsRef<Path>) -> std::io::Result<Config> {
    let contents = std::fs::read_to_string(path)?;
    load_config_str(&contents)
}

pub async fn run(options: RunOptions) -> std::io::Result<()> {
    let mut cfg = options.config;
    cfg.apply_defaults();
    cfg.validate()?;
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

    let dns_table = cfg.dns.as_ref().map(|d| {
        Arc::new(dns::table::IpDomainTable::new(
            d.mapping_capacity,
            d.max_domains_per_ip,
            d.mapping_max_ttl_secs,
        ))
    });
    let resolver = router::resolver::Resolver::new();
    let mut router = router::DefaultRouter::new(
        cfg.router.as_slice(),
        cfg.data.as_ref().unwrap_or(&vec![]).as_slice(),
        resolver,
    )
    .await;
    router.dns_table = dns_table.clone();
    let router = Arc::new(router);
    // Bind before starting business listeners so failures cannot leave a partial service.
    let dns_server = if let (Some(dns_cfg), Some(table)) = (&cfg.dns, &dns_table) {
        Some(
            dns::server::DnsServer::bind(
                dns_cfg.clone(),
                cfg.clone(),
                router.clone(),
                table.clone(),
            )
            .await?,
        )
    } else {
        None
    };

    spawn(observe_registry.sampler_task());
    if let Some(listen_addr) = proxy_observe::env_listen_addr() {
        let observe_router = dns::observe::router(
            observe_registry.clone(),
            dns_table.clone(),
            proxy_observe::env_token(),
        );
        let token = shutdown.clone();
        spawn(async move {
            if let Err(error) = dns::observe::serve(observe_router, listen_addr, token).await {
                error!("observe server failed: {error}");
            }
        });
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
    if let Some(server) = dns_server {
        let token = shutdown.clone();
        tasks.push(spawn(server.run(token)));
    }
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

    let mut failure = None;
    if tasks.is_empty() {
        shutdown.cancelled().await;
    } else {
        let mut tasks_stream = futures::stream::FuturesUnordered::from_iter(tasks);
        tokio::select! {
            _ = shutdown.cancelled() => {
                log::info!("Shutdown signal received, waiting for listeners to finish...");
            }
            Some(result) = futures::StreamExt::next(&mut tasks_stream) => {
                error!(
                    "A listener task finished unexpectedly: {:?}, cancelling remaining listeners...",
                    result
                );
                match result {
                    Ok(Err(e)) => failure = Some(e),
                    Err(e) => failure = Some(std::io::Error::other(e)),
                    Ok(Ok(())) if !shutdown.is_cancelled() => failure = Some(std::io::Error::other("listener stopped unexpectedly")),
                    _ => {}
                }
                shutdown.cancel();
            }
        }

        while let Some(res) = futures::StreamExt::next(&mut tasks_stream).await {
            match res {
                Ok(Err(e)) => {
                    error!("listener failed during shutdown: {e}");
                    if failure.is_none() {
                        failure = Some(e);
                    }
                }
                Err(e) => {
                    error!("listener task error during shutdown: {e}");
                    if failure.is_none() {
                        failure = Some(std::io::Error::other(e));
                    }
                }
                _ => {}
            }
        }
    }

    match failure {
        Some(e) => Err(e),
        None => Ok(()),
    }
}

pub async fn run_config_file(path: impl AsRef<Path>) -> std::io::Result<()> {
    let contents = fs::read_to_string(path).await?;
    let cfg = load_config_str(&contents)?;
    let observe_registry = ObserveRegistry::new();
    let shutdown = CancellationToken::new();

    let shutdown_signal = shutdown.clone();
    spawn(async move {
        let ctrl_c = tokio::signal::ctrl_c();
        #[cfg(unix)]
        {
            let mut sigterm =
                match tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()) {
                    Ok(s) => s,
                    Err(e) => {
                        log::warn!("Failed to register SIGTERM handler: {}", e);
                        let _ = ctrl_c.await;
                        shutdown_signal.cancel();
                        return;
                    }
                };
            tokio::select! {
                _ = ctrl_c => {
                    log::info!("Received SIGINT (Ctrl-C), initiating shutdown...");
                }
                _ = sigterm.recv() => {
                    log::info!("Received SIGTERM, initiating shutdown...");
                }
            }
        }
        #[cfg(not(unix))]
        {
            let _ = ctrl_c.await;
            log::info!("Received Ctrl-C, initiating shutdown...");
        }
        shutdown_signal.cancel();
    });

    run(RunOptions {
        config: cfg,
        observe_registry,
        shutdown,
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

#[cfg(test)]
mod config_validation_tests {
    use super::*;

    fn config_text(protocols: &[&str]) -> String {
        let mut text = "router = []\nconnector = []\n".to_string();
        if protocols.is_empty() {
            text.push_str("listener = []\n");
        }
        for (index, proto) in protocols.iter().enumerate() {
            text.push_str(&format!(
                "\n[[listener]]\nname = \"in_{index}\"\nendpoint = \"0.0.0.0:{}\"\nproto = \"{proto}\"\nrouter = \"main\"\n",
                12345 + index
            ));
        }
        text
    }

    #[test]
    fn config_load_limits_only_tproxy_listeners() {
        for protocols in [vec![], vec!["socks5", "http"], vec!["tproxy", "socks5"]] {
            assert!(load_config_str(&config_text(&protocols)).is_ok());
        }
        let error = load_config_str(&config_text(&["tproxy", "tproxy"])).unwrap_err();
        assert_eq!(error.kind(), std::io::ErrorKind::InvalidInput);
        assert!(error.to_string().contains("only one tproxy"));
    }

    #[tokio::test]
    async fn run_rejects_multiple_tproxy_before_startup() {
        // Direct deserialization bypasses load_config_str; run must still validate.
        let config = toml::from_str(&config_text(&["tproxy", "tproxy"])).unwrap();
        let error = run(RunOptions {
            config,
            observe_registry: ObserveRegistry::new(),
            shutdown: CancellationToken::new(),
        })
        .await
        .unwrap_err();
        assert_eq!(error.kind(), std::io::ErrorKind::InvalidInput);
        assert!(error.to_string().contains("only one tproxy"));
    }
}
