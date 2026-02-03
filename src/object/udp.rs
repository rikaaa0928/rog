use crate::connector;
use crate::def::{RouterSet, RunAcceptor, RunStream, UDPPacket};
use crate::object::config::ObjectConfig;
use crate::util::RunAddr;
use log::{debug, info, warn};
use std::io::Result;
use std::net::SocketAddr;
use std::sync::Arc;
use tokio::net::UdpSocket;
use tokio::{select, spawn};
use tokio_util::sync::CancellationToken;

pub async fn handle_udp_connection(
    mut stream: Box<dyn RunStream>,
    acc: Arc<Box<dyn RunAcceptor>>,
    config: Arc<ObjectConfig>,
    router: Arc<dyn RouterSet>,
    addr: RunAddr,
) -> Result<()> {
    info!("udp? {:?}", addr);
    let udp_socket_base_res = UdpSocket::bind("127.0.0.1:0").await;
    if udp_socket_base_res.is_err() {
        acc.post_handshake(stream.as_mut(), true, 0).await?;
        return Err(udp_socket_base_res.err().unwrap());
    }
    let udp_socket_base = udp_socket_base_res?;
    let udp_port = udp_socket_base.local_addr()?.port();
    acc.post_handshake(stream.as_mut(), false, udp_port).await?;
    let udp_socket_reader = Arc::new(udp_socket_base);
    let udp_socket_writer = Arc::clone(&udp_socket_reader);
    info!("provide {} for {:?}", &udp_port, &addr);

    let cancel_token = CancellationToken::new();

    let token_a = cancel_token.clone();
    let a: tokio::task::JoinHandle<Result<()>> = spawn(async move {
        let mut buf = [0u8; 1];
        loop {
            select! {
                biased;
                _ = token_a.cancelled() => {
                    debug!("UDP TCP read loop (a) interrupted by cancellation.");
                    break;
                }
                read_res = stream.read_exact(&mut buf) => {
                    match read_res {
                        Err(e) => {
                            debug!("UDP TCP read error: {:?}", e);
                            break;
                        }
                        Ok(0) => {
                            warn!("udp tcp read 0, remote closed");
                            break;
                        }
                        Ok(_) => {
                            // Continue reading to detect closure/errors
                        }
                    }
                }
            }
        }
        token_a.cancel();
        debug!("udp tcp done");
        Ok(())
    });

    debug!("udp first packet start");
    let mut buf = [0u8; 65536];
    let res: Result<(usize, SocketAddr)> = udp_socket_reader.recv_from(&mut buf).await;
    if res.is_err() {
        debug!("udp first packet recv error {:?}", res.as_ref().err());
        cancel_token.cancel();
        return Err(res.err().unwrap());
    }
    let (n, src_addr) = res?;
    debug!(
        "udp first packet read src_addr {:?} {} {:?}",
        src_addr,
        n,
        &buf[..n]
    );
    let udp_packet = UDPPacket::parse(&buf[..n], src_addr)?;
    if udp_packet.data.is_empty() {
        warn!("udp first packet drop");
        cancel_token.cancel();
        return Ok(());
    }
    let client_name = router
        .route(
            config.listener.name.as_str(),
            config.listener.router.as_str(),
            &RunAddr {
                addr: udp_packet.meta.dst_addr.clone(),
                port: udp_packet.meta.dst_port,
                udp: false,
            },
        )
        .await;
    let conn_conf = config.connector.get(client_name.as_str()).unwrap();
    let ctor = connector::create(conn_conf).await?;
    let (mut udp_tunnel_reader, udp_tunnal_writer) = ctor
        .udp_tunnel(format!(
            "{}:{}",
            udp_packet.meta.src_addr,
            udp_packet.meta.src_port,
        ))
        .await?
        .unwrap();
    let t_res = udp_tunnal_writer.write(udp_packet).await;
    if t_res.is_err() {
        warn!(
            "udp first packet tunnel write error {:?}",
            t_res.as_ref().err()
        );
        cancel_token.cancel();
        return Err(t_res.err().unwrap());
    }

    debug!("udp loop start");

    let token_b = cancel_token.clone();
    let b: tokio::task::JoinHandle<Result<()>> = spawn(async move {
        let mut buf = [0u8; 65536];
        loop {
            let res: Result<(usize, SocketAddr)> = select! {
                biased;
                _ = token_b.cancelled() => {
                    debug!("UDP loop b interrupted by cancellation.");
                    break;
                },
                recv_res = udp_socket_reader.recv_from(&mut buf) => {
                   recv_res
                }
            };
            match res {
                Err(e) => {
                    debug!("udp loop b udp server recv error {:?}", e);
                    break;
                }
                Ok((n, src_addr)) => {
                    debug!(
                        "udp b server read src_addr {:?} {} {:?}",
                        src_addr,
                        n,
                        &buf[..n]
                    );
                    let udp_packet = match UDPPacket::parse(&buf[..n], src_addr) {
                        Ok(p) => p,
                        Err(e) => {
                            warn!("udp loop b parse error {:?}", e);
                            break;
                        }
                    };
                    if udp_packet.data.is_empty() {
                        warn!("udp drop");
                        continue;
                    }

                    debug!("udp server get udp_packet {:?}", &udp_packet);
                    if let Err(e) = udp_tunnal_writer.write(udp_packet).await {
                        warn!("udp loop b udp tunnel write error {:?}", e);
                        break;
                    }
                }
            }
        }
        token_b.cancel();
        debug!("udp loop b done");
        Ok(())
    });

    let token_c = cancel_token.clone();
    let c: tokio::task::JoinHandle<Result<()>> = spawn(async move {
        'c_job: loop {
            let res: Result<UDPPacket> = select! {
                biased;
                _ = token_c.cancelled() => {
                    debug!("UDP loop c interrupted by cancellation.");
                    break;
                },
                read_res = udp_tunnel_reader.read() => {
                   read_res
                }
            };
            match res {
                Err(e) => {
                    debug!("udp loop c tunnel read error {:?}", e);
                    break;
                }
                Ok(udp_packet) => {
                    let (payloads, src_addr_str, dst_addr) = udp_packet.reply_bytes();
                    debug!(
                        "udp c tunnel udp_packet read src {} {} {:?} \n{:?}",
                        &src_addr_str, &dst_addr, udp_packet, &payloads
                    );

                    for payload in payloads {
                        if let Err(e) = udp_socket_writer
                            .send_to(payload.as_slice(), src_addr_str.clone())
                            .await
                        {
                            warn!("udp loop c udp server send error {:?}", e);
                            break 'c_job;
                        }
                    }
                }
            }
        }
        token_c.cancel();
        debug!("udp loop c done");
        Ok(())
    });

    let _ = a.await;
    let _ = b.await;
    let _ = c.await;
    debug!("udp loop done");
    Ok(())
}
