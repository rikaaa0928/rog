use crate::connector;
use crate::def::{RouterSet, RunAcceptor, RunStream, UDPPacket};
use crate::object::config::ObjectConfig;
use crate::util::RunAddr;
use log::{debug, info, warn};
use std::io::{Error, ErrorKind, Result};
use std::net::SocketAddr;
use std::sync::Arc;
use tokio::net::UdpSocket;
use tokio::sync::Notify;
use tokio::{select, spawn};

pub async fn handle_udp_connection(
    mut stream: Box<dyn RunStream>,
    acc: Arc<Box<dyn RunAcceptor>>,
    config: Arc<ObjectConfig>,
    router: Arc<dyn RouterSet>,
    addr: RunAddr,
    // connector: Arc<Mutex<Box<dyn RunConnector>>>,
) -> Result<()> {
    info!("udp? {:?}", addr);
    // let (mut r, mut w) = stream.split();
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

    let shutdown_notifier = Arc::new(Notify::new());

    let shutdown_notifier_for_a = shutdown_notifier.clone();
    let a: tokio::task::JoinHandle<Result<()>> = spawn(async move {
        let mut buf = [0u8; 1];
        loop {
            select! {
                biased;
                _ = shutdown_notifier_for_a.notified() => {
                    debug!("UDP TCP read loop (a) interrupted by shutdown signal.");
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
        shutdown_notifier_for_a.notify_waiters();
        debug!("udp tcp done");
        Ok(())
    });
    debug!("udp first packet start");
    let mut buf = [0u8; 65536];
    let res: Result<(usize, SocketAddr)> = udp_socket_reader.recv_from(&mut buf).await;
    if res.is_err() {
        debug!("udp first packet recv error {:?}", res.as_ref().err());
        shutdown_notifier.notify_waiters();
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
    if (&udp_packet).data.is_empty() {
        warn!("udp first packet drop");
        shutdown_notifier.notify_waiters();
        return Ok(());
    }
    let client_name = router
        .route(
            config.listener.name.as_str(),
            config.listener.router.as_str(),
            &RunAddr {
                addr: (&udp_packet).meta.dst_addr.clone(),
                port: (&udp_packet).meta.dst_port,
                udp: false,
                // cache: None,
            },
        )
        .await;
    let conn_conf = config.connector.get(client_name.as_str()).unwrap();
    let ctor = connector::create(conn_conf).await?;
    let (mut udp_tunnel_reader, udp_tunnal_writer) = ctor
        .udp_tunnel(format!(
            "{}:{}",
            (&udp_packet).meta.src_addr,
            (&udp_packet).meta.src_port,
        ))
        .await?
        .unwrap();
    let t_res = udp_tunnal_writer.write(udp_packet).await;
    if t_res.is_err() {
        warn!(
            "udp first packet tunnel write error {:?}",
            t_res.as_ref().err()
        );
        shutdown_notifier.notify_waiters();
        return Err(t_res.err().unwrap());
    }

    // let (udp_tunnel_sender, udp_tunnel_receiver) = oneshot::channel();
    // let mut udp_tunnel_sender = Some(udp_tunnel_sender);
    debug!("udp loop start");
    // let udp_socket = Arc::clone(&udp_socket_base);
    // let config_clone_for_b = Arc::clone(&config);
    // let router_clone_for_b = Arc::clone(&router);
    // let connector_clone_for_b = Arc::clone(&connector);
    let shutdown_notifier_for_b = shutdown_notifier.clone();

    let b: tokio::task::JoinHandle<Result<()>> = spawn(async move {
        let mut buf = [0u8; 65536];
        // let mut udp_tunnel: Option<Arc<Box<dyn RunUdpStream>>> = None;
        loop {
            let res: Result<(usize, SocketAddr)> = select! {
                biased;
                _ = shutdown_notifier_for_b.notified() => {
                    debug!("UDP loop b interrupted by shutdown signal.");
                    Err(Error::new(ErrorKind::Interrupted, "shutdown signaled"))
                },
                recv_res = udp_socket_reader.recv_from(&mut buf) => {
                   Ok(recv_res?)
                }
            };
            if res.is_err() {
                debug!("udp loop b udp server recv error {:?}", res.err());
                break;
            }
            let (n, src_addr) = res?;
            debug!(
                "udp b server read src_addr {:?} {} {:?}",
                src_addr,
                n,
                &buf[..n]
            );
            let udp_packet = UDPPacket::parse(&buf[..n], src_addr)?;
            if (&udp_packet).data.is_empty() {
                warn!("udp drop");
                continue;
            }
            // if udp_tunnel.is_none() {
            //     let client_name = router_clone_for_b
            //         .route(
            //             config_clone_for_b.listener.name.as_str(),
            //             config_clone_for_b.listener.router.as_str(),
            //             &RunAddr {
            //                 addr: (&udp_packet).meta.dst_addr.clone(),
            //                 port: (&udp_packet).meta.dst_port,
            //                 udp: false,
            //                 cache: None,
            //             },
            //         )
            //         .await;
            //     let conn_conf = config_clone_for_b
            //         .connector
            //         .get(client_name.as_str())
            //         .unwrap();
            //     let ctor = connector::create(conn_conf).await?;
            //     let t = Arc::new(
            //         ctor.udp_tunnel(format!(
            //             "{}:{}",
            //             (&udp_packet).meta.src_addr,
            //             (&udp_packet).meta.src_port,
            //         ))
            //         .await?
            //         .unwrap(),
            //     );
            //     let _ = udp_tunnel_sender.take().unwrap().send(Arc::clone(&t));
            //     udp_tunnel.replace(Arc::clone(&t));
            // }

            debug!("udp server get udp_packet {:?}", &udp_packet);
            // let udp_tunnel_ref = udp_tunnel.as_ref().unwrap();
            let res = udp_tunnal_writer.write(udp_packet).await;
            if res.is_err() {
                warn!("udp loop b udp tunnel write error {:?}", res.err());
                break;
            }
        }
        shutdown_notifier_for_b.notify_waiters();
        debug!("udp loop b done");
        Ok(())
    });
    // let udp_socket_c = Arc::clone(&udp_socket_base);
    let shutdown_notifier_for_c = shutdown_notifier.clone();
    let c: tokio::task::JoinHandle<Result<()>> = spawn(async move {
        // let udp_tunnel_res = udp_tunnel_receiver.await;
        // if udp_tunnel_res.is_err() {
        //     debug!("udp_tunnel_receiver c error {:?}", udp_tunnel_res.err());
        //     shutdown_notifier_for_c.notify_waiters();
        //     return Ok(());
        // }
        // let udp_tunnel = udp_tunnel_res.unwrap();
        'c_job: loop {
            let res: Result<UDPPacket> = select! {
                biased;
                _ = shutdown_notifier_for_c.notified() => {
                    debug!("UDP loop c interrupted by shutdown signal.");
                    Err(Error::new(ErrorKind::Interrupted, "shutdown signaled"))
                },
                read_res = udp_tunnel_reader.read() => {
                   Ok(read_res?)
                }
            };
            if res.is_err() {
                debug!("udp loop c tunnel read error {:?}", res.err());
                break;
            }
            let udp_packet = res?;
            let (payloads, src_addr_str, dst_addr) = udp_packet.reply_bytes();
            debug!(
                "udp c tunnel udp_packet read src {} {} {:?} \n{:?}",
                &src_addr_str, &dst_addr, udp_packet, &payloads
            );

            for payload in payloads {
                let res = udp_socket_writer
                    .send_to(payload.as_slice(), src_addr_str.clone())
                    .await;
                if res.is_err() {
                    warn!("udp loop c udp server send error {:?}", res.err());
                    break 'c_job;
                }
            }
        }
        shutdown_notifier_for_c.notify_waiters();
        debug!("udp loop c done");
        Ok(())
    });
    let _ = a.await;
    let _ = b.await;
    let _ = c.await;
    debug!("udp loop done");
    Ok(())
}
