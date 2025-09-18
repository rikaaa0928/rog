use crate::def::{RouterSet, RunAccStream, RunConnector};
use crate::object::config::ObjectConfig;
use crate::{connector, listener};
use log::{debug, error};
use std::collections::HashMap; // New
use std::io;
use std::io::Error;
use std::sync::Arc;
use tokio::spawn;
use tokio::sync::Mutex; // Already present, but ensure it's used for cache

pub mod config;
pub mod tcp;
pub mod udp;
pub mod raw_udp;

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
        let acc = listener::create(&config_outer, router_outer.clone()).await?;
        let main_acceptor = Arc::new(acc);
        let connector_cache_outer = self.connector_cache.clone(); // Clone cache Arc for the loop

        loop {
            let (acc_stream, _) = main_acceptor.accept().await?;
            let main_acceptor_clone = Arc::clone(&main_acceptor);
            let router_clone = Arc::clone(&router_outer);
            let config_clone = Arc::clone(&config_outer);
            let cache_clone = Arc::clone(&connector_cache_outer); // Clone cache Arc for the spawned task

            spawn(async move {
                match acc_stream {
                    RunAccStream::TCPStream(tcp_stream) => {
                        let (mut r, mut w) = tcp_stream.split();
                        let addr_res = main_acceptor_clone.handshake(r.as_mut(), w.as_mut()).await;
                        match addr_res {
                            Err(e) => {
                                error!("Handshake error: {}", e);
                            }
                            Ok(addr) => {
                                let addr_ref = &addr;

                                if addr_ref.udp {
                                    // Assuming udp::handle_udp_connection might also need caching if it creates connectors.
                                    // For now, this part remains as is, focusing on the primary TCP/raw_udp paths.
                                    udp::handle_udp_connection(
                                        r,
                                        w,
                                        Arc::clone(&main_acceptor_clone),
                                        config_clone.clone(), // Pass cloned config
                                        router_clone.clone(), // Pass cloned router
                                        addr,
                                    )
                                    .await?;
                                } else {
                                    let client_name = router_clone
                                        .route(
                                            config_clone.listener.name.as_str(),
                                            config_clone.listener.router.as_str(),
                                            addr_ref,
                                        )
                                        .await;
                                    let conn_conf = config_clone.connector.get(client_name.as_str())
                                        .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, format!("Connector config '{}' not found", client_name)))?;

                                    let connector_obj: Arc<Box<dyn RunConnector>>;
                                    {
                                        let mut cache_guard = cache_clone.lock().await;
                                        if let Some(cached_connector) = cache_guard.get(client_name.as_str()) {
                                            connector_obj = Arc::clone(cached_connector);
                                            debug!("Reusing cached connector for client: {}", client_name);
                                        } else {
                                            debug!("Creating new connector for client: {}", client_name);
                                            let new_connector = connector::create(conn_conf).await?;
                                            let new_connector_arc = Arc::new(new_connector);
                                            cache_guard.insert(client_name.clone(), Arc::clone(&new_connector_arc));
                                            connector_obj = new_connector_arc;
                                        }
                                    }

                                    debug!("Handshake successful {:?}", addr_ref);
                                    let client_stream_res = Arc::clone(&connector_obj)
                                        .connect(addr_ref.endpoint())
                                        .await;
                                    let mut error_occurred = false; // Renamed for clarity
                                    if client_stream_res.is_err() {
                                        error_occurred = true;
                                    }
                                    debug!("object connect successful {:?} {:?}", addr_ref,&error_occurred);
                                    let client_stream = client_stream_res?;
                                    main_acceptor_clone
                                        .post_handshake(r.as_mut(), w.as_mut(), error_occurred, 0)
                                        .await?;
                                    tcp::handle_tcp_connection(r, w, addr, client_stream).await?;
                                }
                            }
                        }
                        Ok::<(), Error>(())
                    }
                    RunAccStream::UDPSocket((r, w)) => {
                        // Pass the cloned cache to raw_udp handling
                        raw_udp::handle_raw_udp(r, w, config_clone, router_clone, cache_clone).await
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
