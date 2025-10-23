use crate::connector;
use crate::def::{RouterSet, RunConnector, RunUdpReader, RunUdpWriter, UDPPacket}; // Added RunConnector
use crate::object::config::ObjectConfig;
use crate::util::RunAddr;
use log::{debug, warn};
use std::collections::HashMap; // Added HashMap
use std::io::{self, Error, ErrorKind, Result}; // Ensure io is imported for io::Error::new
use std::sync::Arc;
use tokio::sync::{Mutex, Notify}; // Added Mutex
use tokio::{select, spawn};

pub async fn handle_raw_udp(
    mut r: Box<dyn RunUdpReader>,
    w: Box<dyn RunUdpWriter>,
    config: Arc<ObjectConfig>,
    router: Arc<dyn RouterSet>,
    connector_cache: Arc<Mutex<HashMap<String, Arc<Box<dyn RunConnector>>>>>, // New argument
) -> Result<()> {
    debug!("raw udp, route based on the first packet");
    let first_packet = r.read().await?;
    let client_name = router
        .route(
            config.listener.name.as_str(),
            config.listener.router.as_str(),
            &RunAddr {
                addr: first_packet.meta.dst_addr.clone(),
                port: first_packet.meta.dst_port,
                udp: true, // Set to true for UDP context
                           // cache: None,
            },
        )
        .await;

    let conn_conf = config.connector.get(client_name.as_str()).ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::NotFound,
            format!("Connector config '{}' not found for UDP", client_name),
        )
    })?;

    let connector_obj: Arc<Box<dyn RunConnector>>;
    {
        let mut cache_guard = connector_cache.lock().await;
        if let Some(cached_connector) = cache_guard.get(client_name.as_str()) {
            connector_obj = Arc::clone(cached_connector);
            debug!("Reusing cached connector for UDP: {}", client_name);
        } else {
            debug!("Creating new connector for UDP: {}", client_name);
            let new_connector = connector::create(conn_conf).await?;
            let new_connector_arc = Arc::new(new_connector);
            cache_guard.insert(client_name.clone(), Arc::clone(&new_connector_arc));
            connector_obj = new_connector_arc;
        }
    }

    let (mut udp_reader, udp_writer) = connector_obj
        .udp_tunnel(format!(
            "{}:{}",
            first_packet.meta.src_addr,
            first_packet.meta.src_port,
        ))
        .await?
        .ok_or_else(|| {
            io::Error::other(
                "UDP tunnel creation failed or not supported by connector",
            )
        })?;

    udp_writer.write(first_packet).await?;

    let shutdown_notifier = Arc::new(Notify::new());

    debug!("raw udp loop start");

    let shutdown_notifier_for_b = shutdown_notifier.clone();
    // let udp_tunnal_b=Arc::clone(&udp_tunnel);
    let b: tokio::task::JoinHandle<Result<()>> = spawn(async move {
        loop {
            let res: Result<UDPPacket> = select! {
                biased;
                _ = shutdown_notifier_for_b.notified() => {
                    debug!("raw UDP loop b interrupted by shutdown signal.");
                    Err(Error::new(ErrorKind::Interrupted, "shutdown signaled"))
                },
                recv_res = r.read() => {
                   recv_res
                }
            };
            if res.is_err() {
                debug!("raw udp loop b read error {:?}", res.err());
                break;
            }
            let packet = res?;
            debug!(
                "raw udp loop b read src_addr {:?} {} {:?} {}",
                packet.meta.src_addr,
                packet.meta.src_port,
                packet.meta.dst_addr,
                packet.meta.dst_port
            );
            let udp_packet = packet;
            if udp_packet.data.is_empty() {
                warn!("raw udp drop");
                continue;
            }

            debug!("raw udp server get udp_packet {:?}", &udp_packet);
            let udp_tunnel_ref = udp_writer.as_ref();
            let res = udp_tunnel_ref.write(udp_packet).await;
            if res.is_err() {
                warn!("raw udp loop b udp tunnel write error {:?}", res.err());
                break;
            }
        }
        shutdown_notifier_for_b.notify_waiters();
        debug!("raw udp loop b done");
        Ok(())
    });
    let shutdown_notifier_for_c = shutdown_notifier.clone();
    // let udp_tunnal_c=Arc::clone(&udp_tunnel);
    let c: tokio::task::JoinHandle<Result<()>> = spawn(async move {
        loop {
            let res: Result<UDPPacket> = select! {
                biased;
                _ = shutdown_notifier_for_c.notified() => {
                    debug!("raw UDP loop c interrupted by shutdown signal.");
                    Err(Error::new(ErrorKind::Interrupted, "shutdown signaled"))
                },
                read_res = udp_reader.read() => {
                   Ok(read_res?)
                }
            };
            if res.is_err() {
                debug!("raw udp loop c tunnel read error {:?}", res.err());
                break;
            }
            let udp_packet = res?;
            debug!(
                "raw udp tunnel udp_packet read src {:?} {:?}",
                udp_packet.meta.src_addr,
                udp_packet.meta.src_port,
            );
            w.write(udp_packet).await?;
        }
        shutdown_notifier_for_c.notify_waiters();
        debug!("raw udp loop c done");
        Ok(())
    });
    let _ = b.await;
    let _ = c.await;
    debug!("raw udp loop done");
    Ok(())
}
