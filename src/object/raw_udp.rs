use crate::connector;
use crate::def::{RouterSet, RunConnector, RunUdpReader, RunUdpWriter, UDPPacket};
use crate::object::config::ObjectConfig;
use crate::util::{RunAddr, format_endpoint};
use log::{debug, warn};
use proxy_observe::{ConnectionMeta, ObserveRegistry};
use std::collections::HashMap;
use std::io::{self, Result};
use std::sync::Arc;
use tokio::sync::Mutex;
use tokio::{select, spawn};
use tokio_util::sync::CancellationToken;

pub async fn handle_raw_udp(
    mut r: Box<dyn RunUdpReader>,
    w: Box<dyn RunUdpWriter>,
    config: Arc<ObjectConfig>,
    router: Arc<dyn RouterSet>,
    connector_cache: Arc<Mutex<HashMap<String, Arc<Box<dyn RunConnector>>>>>,
    observe_registry: ObserveRegistry,
) -> Result<()> {
    debug!("raw udp, route based on the first packet");
    let mut first_packet = r.read().await?;
    let orig_dst_addr = first_packet.meta.dst_addr.clone();
    let (client_name, mapped_domain) = router
        .route_with_domain(
            config.listener.name.as_str(),
            config.listener.router.as_str(),
            &RunAddr {
                addr: first_packet.meta.dst_addr.clone(),
                port: first_packet.meta.dst_port,
                udp: true,
            },
            &crate::def::RouteContext {
                client_ip: first_packet.meta.src_addr.parse().ok(),
                use_dns_mapping: config.listener.proto == "tproxy",
            },
        )
        .await;

    let conn_conf = config.connector.get(client_name.as_str()).ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::NotFound,
            format!("Connector config '{}' not found for UDP", client_name),
        )
    })?;
    let rewrite_v6_domain =
        if conn_conf.proto != "tcp" && orig_dst_addr.parse::<std::net::Ipv6Addr>().is_ok() {
            mapped_domain
        } else {
            None
        };

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

    let observe = observe_registry.open(ConnectionMeta {
        service: "rog".to_string(),
        network: "udp".to_string(),
        listener: config.listener.name.clone(),
        router: Some(config.listener.router.clone()),
        route: Some(client_name.clone()),
        inbound: Some(config.listener.name.clone()),
        outbound: Some(client_name.clone()),
        source: format_endpoint(&first_packet.meta.src_addr, first_packet.meta.src_port),
        destination: format_endpoint(&first_packet.meta.dst_addr, first_packet.meta.dst_port),
        site: None,
    });

    let src_endpoint = format_endpoint(&first_packet.meta.src_addr, first_packet.meta.src_port);
    let (mut udp_reader, udp_writer) =
        connector_obj
            .udp_tunnel(src_endpoint)
            .await?
            .ok_or_else(|| {
                io::Error::other("UDP tunnel creation failed or not supported by connector")
            })?;

    let first_packet_len = first_packet.data.len() as u64;
    if let Some(ref domain) = rewrite_v6_domain {
        first_packet.meta.dst_addr = domain.clone();
    }
    udp_writer.write(first_packet).await?;
    observe.add_tx(first_packet_len);

    let cancel_token = CancellationToken::new();

    debug!("raw udp loop start");

    let token_b = cancel_token.clone();
    let observe_tx = observe.clone();
    let rewrite_domain_b = rewrite_v6_domain.clone();
    let b: tokio::task::JoinHandle<Result<()>> = spawn(async move {
        loop {
            let res: Result<UDPPacket> = select! {
                biased;
                _ = token_b.cancelled() => {
                    debug!("raw UDP loop b interrupted by cancellation.");
                    break;
                },
                recv_res = r.read() => {
                   recv_res
                }
            };
            match res {
                Err(e) => {
                    debug!("raw udp loop b read error {:?}", e);
                    break;
                }
                Ok(mut packet) => {
                    debug!(
                        "raw udp loop b read src_addr {:?} {} {:?} {}",
                        packet.meta.src_addr,
                        packet.meta.src_port,
                        packet.meta.dst_addr,
                        packet.meta.dst_port
                    );
                    if packet.data.is_empty() {
                        warn!("raw udp drop");
                        continue;
                    }
                    if let Some(ref domain) = rewrite_domain_b {
                        packet.meta.dst_addr = domain.clone();
                    }

                    debug!("raw udp server get udp_packet {:?}", &packet);
                    let packet_len = packet.data.len() as u64;
                    let udp_tunnel_ref = udp_writer.as_ref();
                    if let Err(e) = udp_tunnel_ref.write(packet).await {
                        warn!("raw udp loop b udp tunnel write error {:?}", e);
                        break;
                    }
                    observe_tx.add_tx(packet_len);
                }
            }
        }
        token_b.cancel();
        debug!("raw udp loop b done");
        Ok(())
    });

    let token_c = cancel_token.clone();
    let observe_rx = observe.clone();
    let restore_v6_addr = rewrite_v6_domain.as_ref().map(|_| orig_dst_addr);
    let c: tokio::task::JoinHandle<Result<()>> = spawn(async move {
        loop {
            let res: Result<UDPPacket> = select! {
                biased;
                _ = token_c.cancelled() => {
                    debug!("raw UDP loop c interrupted by cancellation.");
                    break;
                },
                read_res = udp_reader.read() => {
                   read_res
                }
            };
            match res {
                Err(e) => {
                    debug!("raw udp loop c tunnel read error {:?}", e);
                    break;
                }
                Ok(mut udp_packet) => {
                    let packet_len = udp_packet.data.len() as u64;
                    if let Some(ref orig_ip) = restore_v6_addr {
                        udp_packet.meta.dst_addr = orig_ip.clone();
                    }
                    debug!(
                        "raw udp tunnel udp_packet read src {:?} {:?}",
                        udp_packet.meta.src_addr, udp_packet.meta.src_port,
                    );
                    if let Err(e) = w.write(udp_packet).await {
                        warn!("raw udp loop c write error {:?}", e);
                        break;
                    }
                    observe_rx.add_rx(packet_len);
                }
            }
        }
        token_c.cancel();
        debug!("raw udp loop c done");
        Ok(())
    });

    let _ = b.await;
    let _ = c.await;
    debug!("raw udp loop done");
    Ok(())
}
