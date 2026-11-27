use crate::def::{RouterSet, RunAccStream, RunConnector};
use crate::object::config::ObjectConfig;
use crate::{connector, listener};
use log::{debug, error};
use proxy_observe::{ConnectionMeta, ObserveRegistry};
use std::collections::HashMap;
use std::io;
use std::io::Error;
use std::sync::Arc;
use tokio::spawn;
use tokio::sync::Mutex;
use tokio_util::sync::CancellationToken;
// Already present, but ensure it's used for cache

pub mod config;
pub mod raw_udp;
pub mod tcp;
pub mod udp;

pub struct Object {
    config: Arc<ObjectConfig>,
    router: Arc<dyn RouterSet>,
    connector_cache: Arc<Mutex<HashMap<String, Arc<Box<dyn RunConnector>>>>>, // New field
    observe_registry: ObserveRegistry,
}

impl Object {
    pub fn new(
        config: Arc<ObjectConfig>,
        router: Arc<dyn RouterSet>,
        observe_registry: ObserveRegistry,
    ) -> Self {
        Self {
            config,
            router,
            connector_cache: Arc::new(Mutex::new(HashMap::new())), // Initialize cache
            observe_registry,
        }
    }

    pub async fn start(&self) -> io::Result<()> {
        self.start_with_shutdown(CancellationToken::new()).await
    }

    pub async fn start_with_shutdown(&self, shutdown: CancellationToken) -> io::Result<()> {
        let config_outer = self.config.clone(); // Renamed for clarity
        let router_outer = self.router.clone(); // Renamed for clarity
        let sniff_cfg = Arc::new(crate::util::sniff::SniffConfig::from_listener(
            &config_outer.listener,
        ));
        let acc = listener::create(&config_outer, shutdown.clone())
            .await
            .map_err(|e| {
                error!("Failed to create listener: {}", e);
                e
            })?;

        let main_acceptor = Arc::new(acc);
        let connector_cache_outer = self.connector_cache.clone(); // Clone cache Arc for the loop

        loop {
            let (acc_stream, peer_addr) = tokio::select! {
            _ = shutdown.cancelled() => return Ok(()),
            accepted = main_acceptor.accept() => match accepted {
                Ok(item) => item,
                Err(e) => {
                    if shutdown.is_cancelled() {
                        return Ok(());
                    }
                    error!("Failed to accept connection: {}", e);
                    return Err(e);
                }
                },
            };
            let main_acceptor_clone = Arc::clone(&main_acceptor);
            let router_clone = Arc::clone(&router_outer);
            let config_clone = Arc::clone(&config_outer);
            let sniff_cfg_clone = Arc::clone(&sniff_cfg);
            let connector_cache_clone = Arc::clone(&connector_cache_outer); // Clone cache Arc for the spawned task
            let observe_registry_clone = self.observe_registry.clone();
            spawn(async move {
                match acc_stream {
                    RunAccStream::TCPStream(mut tcp_stream) => {
                        let addr_res = main_acceptor_clone.handshake(tcp_stream.as_mut()).await;
                        match addr_res {
                            Err(e) => {
                                error!("Handshake error: {}", e);
                            }
                            Ok((addr, mut payload_cache)) => {
                                let addr_ref = &addr;
                                if addr_ref.udp {
                                    if let Err(e) = udp::handle_udp_connection(
                                        tcp_stream,
                                        Arc::clone(&main_acceptor_clone),
                                        config_clone.clone(),
                                        router_clone.clone(),
                                        addr,
                                        connector_cache_clone,
                                        observe_registry_clone.clone(),
                                    )
                                    .await
                                    {
                                        error!("Error handling UDP connection: {}", e);
                                    }
                                } else {
                                    let mut sniffed_domain = None;
                                    if config_clone.listener.proto == "tproxy"
                                        && payload_cache.is_none()
                                        && sniff_cfg_clone.supports_port(addr_ref.port)
                                    {
                                        let should_sniff = match sniff_cfg_clone.mode {
                                            crate::util::sniff::SniffMode::Off => false,
                                            crate::util::sniff::SniffMode::Always => true,
                                            crate::util::sniff::SniffMode::Ambiguous => {
                                                router_clone.is_ambiguous_ip(
                                                    config_clone.listener.router.as_str(),
                                                    Some(peer_addr.ip()),
                                                    addr_ref.addr.as_str(),
                                                )
                                            }
                                        };
                                        if should_sniff {
                                            let (domain, sniff_buf) =
                                                crate::util::sniff::sniff_tcp_stream(
                                                    tcp_stream.as_mut(),
                                                    sniff_cfg_clone.timeout,
                                                )
                                                .await;
                                            sniffed_domain = domain;
                                            if let Some(buf) = sniff_buf {
                                                payload_cache = Some(buf);
                                            }
                                        }
                                    }
                                    let (client_name, mapped_domain) = router_clone
                                        .route_with_domain(
                                            config_clone.listener.name.as_str(),
                                            config_clone.listener.router.as_str(),
                                            addr_ref,
                                            &crate::def::RouteContext {
                                                client_ip: Some(peer_addr.ip()),
                                                client_port: Some(peer_addr.port()),
                                                use_dns_mapping: config_clone.listener.proto
                                                    == "tproxy",
                                                sniffed_domain: sniffed_domain.clone(),
                                            },
                                        )
                                        .await;
                                    let conn_conf = match config_clone
                                        .connector
                                        .get(client_name.as_str())
                                    {
                                        Some(c) => c,
                                        None => {
                                            error!("Connector config '{}' not found", client_name);
                                            return Ok(()); // Exit the task for this connection
                                        }
                                    };

                                    let connector_obj: Arc<Box<dyn RunConnector>>;
                                    {
                                        let mut connector_cache_guard =
                                            connector_cache_clone.lock().await;
                                        if let Some(cached_connector) =
                                            connector_cache_guard.get(client_name.as_str())
                                        {
                                            connector_obj = Arc::clone(cached_connector);
                                            debug!(
                                                "Reusing cached connector for client: {}",
                                                client_name
                                            );
                                        } else {
                                            debug!(
                                                "Creating new connector for client: {}",
                                                client_name
                                            );
                                            let new_connector =
                                                match connector::create(conn_conf).await {
                                                    Ok(c) => c,
                                                    Err(e) => {
                                                        error!(
                                                            "Failed to create connector '{}': {}",
                                                            client_name, e
                                                        );
                                                        return Ok(());
                                                    }
                                                };
                                            let new_connector_arc = Arc::new(new_connector);
                                            connector_cache_guard.insert(
                                                client_name.clone(),
                                                Arc::clone(&new_connector_arc),
                                            );
                                            connector_obj = new_connector_arc;
                                        }
                                    }

                                    debug!("Handshake successful {:?}", addr_ref);
                                    let connect_target = resolve_connect_target(
                                        addr_ref,
                                        sniffed_domain.as_deref(),
                                        sniff_cfg_clone.override_destination,
                                    );
                                    let client_stream_res =
                                        Arc::clone(&connector_obj).connect(connect_target).await;

                                    let error_occurred = client_stream_res.is_err();
                                    debug!(
                                        "object connect successful {:?} {:?}",
                                        addr_ref, !error_occurred
                                    );

                                    let client_stream = match client_stream_res {
                                        Ok(s) => s,
                                        Err(e) => {
                                            error!(
                                                "Connector '{}' failed to connect to {:?}: {}",
                                                client_name,
                                                addr_ref.endpoint(),
                                                e
                                            );
                                            // We still need to run post_handshake to inform the client
                                            if let Err(e) = main_acceptor_clone
                                                .post_handshake(tcp_stream.as_mut(), true, 0)
                                                .await
                                            {
                                                error!(
                                                    "Error in post_handshake after connection failure: {}",
                                                    e
                                                );
                                            }
                                            return Ok(());
                                        }
                                    };

                                    if let Err(e) = main_acceptor_clone
                                        .post_handshake(tcp_stream.as_mut(), false, 0)
                                        .await
                                    {
                                        error!("Error in post_handshake: {}", e);
                                        return Ok(());
                                    }
                                    // let (mut r, mut w) = tcp_stream.split();
                                    let observe = observe_registry_clone.open(ConnectionMeta {
                                        service: "rog".to_string(),
                                        network: "tcp".to_string(),
                                        listener: config_clone.listener.name.clone(),
                                        router: Some(config_clone.listener.router.clone()),
                                        route: Some(client_name.clone()),
                                        inbound: Some(config_clone.listener.name.clone()),
                                        outbound: Some(client_name.clone()),
                                        source: peer_addr.to_string(),
                                        destination: addr_ref.endpoint(),
                                        site: sniffed_domain.or(mapped_domain),
                                    });
                                    if let Err(e) = tcp::handle_tcp_connection(
                                        addr,
                                        payload_cache,
                                        client_stream,
                                        tcp_stream,
                                        Some(observe),
                                    )
                                    .await
                                    {
                                        if matches!(
                                            e.kind(),
                                            std::io::ErrorKind::ConnectionReset
                                                | std::io::ErrorKind::BrokenPipe
                                                | std::io::ErrorKind::ConnectionAborted
                                                | std::io::ErrorKind::UnexpectedEof
                                        ) {
                                            debug!("TCP connection closed by peer: {}", e);
                                        } else {
                                            error!("Error handling TCP connection: {}", e);
                                        }
                                    }
                                }
                            }
                        }
                        Ok::<(), Error>(()) // This Ok is for the spawn block's Result type
                    }
                    RunAccStream::UDPSocket((r, w)) => {
                        // Pass the cloned cache to raw_udp handling
                        if let Err(e) = raw_udp::handle_raw_udp(
                            r,
                            w,
                            config_clone,
                            sniff_cfg_clone,
                            router_clone,
                            connector_cache_clone,
                            observe_registry_clone,
                        )
                        .await
                        {
                            error!("Error handling raw UDP: {}", e);
                        }
                        Ok::<(), Error>(())
                    }
                }
            });
        }
    }
}

fn resolve_connect_target(
    addr: &crate::util::RunAddr,
    sniffed_domain: Option<&str>,
    override_destination: bool,
) -> String {
    if override_destination {
        if let Some(domain) = sniffed_domain {
            return crate::util::format_endpoint(domain, addr.port);
        }
    }
    addr.endpoint()
}

#[cfg(test)]
mod tests {
    use super::resolve_connect_target;
    use crate::util::RunAddr;

    #[test]
    fn test_resolve_connect_target_preserves_ip_unless_override_enabled() {
        let v4 = RunAddr {
            addr: "23.227.39.200".to_string(),
            port: 443,
            udp: false,
        };
        assert_eq!(
            resolve_connect_target(&v4, Some("shopify-assets.shopifycdn.com"), false),
            "23.227.39.200:443"
        );
        assert_eq!(
            resolve_connect_target(&v4, Some("shopify-assets.shopifycdn.com"), true),
            "shopify-assets.shopifycdn.com:443"
        );

        let v6 = RunAddr {
            addr: "2620:127:f00e:ff01::".to_string(),
            port: 443,
            udp: false,
        };
        assert_eq!(
            resolve_connect_target(&v6, Some("shopify-assets.shopifycdn.com"), false),
            "[2620:127:f00e:ff01::]:443"
        );
        assert_eq!(
            resolve_connect_target(&v6, Some("shopify-assets.shopifycdn.com"), true),
            "shopify-assets.shopifycdn.com:443"
        );
    }
}
