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
    sniff_cfg: Arc<crate::util::sniff::SniffConfig>,
    router: Arc<dyn RouterSet>,
    connector_cache: Arc<Mutex<HashMap<String, Arc<Box<dyn RunConnector>>>>>,
    observe_registry: ObserveRegistry,
) -> Result<()> {
    debug!("raw udp, route based on the first packet");
    let first_packet = r.read().await?;
    let src_ip = first_packet.meta.src_addr.parse().ok();
    let src_port = first_packet.meta.src_port;
    let dst_ip = first_packet.meta.dst_addr.clone();
    let dst_port = first_packet.meta.dst_port;

    let mut sniffed_domain = None;
    let mut buffered_packets = Vec::new();
    if config.listener.proto == "tproxy" && sniff_cfg.supports_port(dst_port) {
        let should_sniff = match sniff_cfg.mode {
            crate::util::sniff::SniffMode::Off => false,
            crate::util::sniff::SniffMode::Always => true,
            crate::util::sniff::SniffMode::Ambiguous => router.is_ambiguous_ip(
                config.listener.router.as_str(),
                src_ip,
                dst_ip.as_str(),
            ),
        };
        if should_sniff {
            let (domain, pkts) = crate::util::sniff::sniff_udp_quic_packets(
                r.as_mut(),
                first_packet,
                sniff_cfg.timeout,
            )
            .await;
            sniffed_domain = domain;
            buffered_packets = pkts;
        } else {
            buffered_packets.push(first_packet);
        }
    } else {
        buffered_packets.push(first_packet);
    }

    let (client_name, mapped_domain) = router
        .route_with_domain(
            config.listener.name.as_str(),
            config.listener.router.as_str(),
            &RunAddr {
                addr: dst_ip.clone(),
                port: dst_port,
                udp: true,
            },
            &crate::def::RouteContext {
                client_ip: src_ip,
                client_port: Some(src_port),
                use_dns_mapping: config.listener.proto == "tproxy",
                sniffed_domain: sniffed_domain.clone(),
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

    let observe = observe_registry.open(ConnectionMeta {
        service: "rog".to_string(),
        network: "udp".to_string(),
        listener: config.listener.name.clone(),
        router: Some(config.listener.router.clone()),
        route: Some(client_name.clone()),
        inbound: Some(config.listener.name.clone()),
        outbound: Some(client_name.clone()),
        source: format_endpoint(&buffered_packets[0].meta.src_addr, src_port),
        destination: format_endpoint(&dst_ip, dst_port),
        site: sniffed_domain.or(mapped_domain),
    });

    let src_endpoint = format_endpoint(&buffered_packets[0].meta.src_addr, src_port);
    let (mut udp_reader, udp_writer) =
        connector_obj
            .udp_tunnel(src_endpoint)
            .await?
            .ok_or_else(|| {
                io::Error::other("UDP tunnel creation failed or not supported by connector")
            })?;

    for pkt in buffered_packets {
        if pkt.data.is_empty() {
            continue;
        }
        let pkt_len = pkt.data.len() as u64;
        udp_writer.write(pkt).await?;
        observe.add_tx(pkt_len);
    }

    let cancel_token = CancellationToken::new();

    debug!("raw udp loop start");

    let token_b = cancel_token.clone();
    let observe_tx = observe.clone();
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
                Ok(packet) => {
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
                Ok(udp_packet) => {
                    let packet_len = udp_packet.data.len() as u64;
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
