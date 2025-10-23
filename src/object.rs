use crate::def::{RouterSet, RunAccStream, RunConnector};
use crate::object::config::ObjectConfig;
use crate::{connector, listener};
use log::{debug, error};
use std::collections::HashMap;
use std::io;
use std::io::Error;
use std::sync::Arc;
use tokio::spawn;
use tokio::sync::Mutex; // Already present, but ensure it's used for cache

pub mod config;
pub mod raw_udp;
pub mod tcp;
pub mod udp;

pub struct Object {
    config: Arc<ObjectConfig>,
    router: Arc<dyn RouterSet>,
    connector_cache: Arc<Mutex<HashMap<String, Arc<Box<dyn RunConnector>>>>>, // New field
}

impl Object {
    pub fn new(config: Arc<ObjectConfig>, router: Arc<dyn RouterSet>) -> Self {
        Self {
            config,
            router,
            connector_cache: Arc::new(Mutex::new(HashMap::new())), // Initialize cache
        }
    }

    pub async fn start(&self) -> io::Result<()> {
        let config_outer = self.config.clone(); // Renamed for clarity
        let router_outer = self.router.clone(); // Renamed for clarity
        let acc = listener::create(&config_outer, router_outer.clone())
            .await
            .map_err(|e| {
                error!("Failed to create listener: {}", e);
                e
            })?;
        let main_acceptor = Arc::new(acc);
        let connector_cache_outer = self.connector_cache.clone(); // Clone cache Arc for the loop

        loop {
            let (acc_stream, _) = main_acceptor.accept().await.map_err(|e| {
                error!("Failed to accept connection: {}", e);
                e
            })?;
            let main_acceptor_clone = Arc::clone(&main_acceptor);
            let router_clone = Arc::clone(&router_outer);
            let config_clone = Arc::clone(&config_outer);
            let connector_cache_clone = Arc::clone(&connector_cache_outer); // Clone cache Arc for the spawned task

            spawn(async move {
                match acc_stream {
                    RunAccStream::TCPStream(mut tcp_stream) => {
                        let addr_res = main_acceptor_clone.handshake(tcp_stream.as_mut()).await;
                        match addr_res {
                            Err(e) => {
                                error!("Handshake error: {}", e);
                            }
                            Ok((addr, payload_cache)) => {
                                let addr_ref = &addr;
                                if addr_ref.udp {
                                    // Assuming udp::handle_udp_connection might also need caching if it creates connectors.
                                    // For now, this part remains as is, focusing on the primary TCP/raw_udp paths.
                                    if let Err(e) = udp::handle_udp_connection(
                                        tcp_stream,
                                        Arc::clone(&main_acceptor_clone),
                                        config_clone.clone(), // Pass cloned config
                                        router_clone.clone(), // Pass cloned router
                                        addr,
                                    )
                                    .await
                                    {
                                        error!("Error handling UDP connection: {}", e);
                                    }
                                } else {
                                    let client_name = router_clone
                                        .route(
                                            config_clone.listener.name.as_str(),
                                            config_clone.listener.router.as_str(),
                                            addr_ref,
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
                                    let client_stream_res = Arc::clone(&connector_obj)
                                        .connect(addr_ref.endpoint())
                                        .await;

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
                                                error!("Error in post_handshake after connection failure: {}", e);
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
                                    if let Err(e) = tcp::handle_tcp_connection(
                                        addr,
                                        payload_cache,
                                        client_stream,
                                        tcp_stream,
                                    )
                                    .await
                                    {
                                        error!("Error handling TCP connection: {}", e);
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
                            router_clone,
                            connector_cache_clone,
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
        // The loop is infinite, so Ok(()) might not be reachable here unless the loop is broken.
        // If the server is meant to run indefinitely, this Ok(()) might be removed or placed after a loop break condition.
        // For now, keeping it as it was, assuming there's a broader context for server shutdown.
        // Ok(())
    }
}
