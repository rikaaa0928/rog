use crate::def::{RouterSet, RunConnector, RunUdpStream, UDPPacket, RunReadHalf, RunWriteHalf, RunAcceptor};
use crate::object::config::ObjectConfig;
use crate::router::DefaultRouter;
use crate::util::RunAddr;
use crate::{connector, listener};
use log::{debug, warn};
use std::io;
use std::io::{Error, ErrorKind, Result};
use std::net::SocketAddr;
use std::sync::Arc;
use tokio::net::UdpSocket;
use tokio::sync::{oneshot, Mutex};
use tokio::{select, spawn};

pub async fn handle_udp_connection(
    mut r: Box<dyn RunReadHalf>,
    mut w: Box<dyn RunWriteHalf>,
    acc: Arc<Box<dyn RunAcceptor>>,
    config: Arc<ObjectConfig>,
    router: Arc<dyn RouterSet>,
    addr: RunAddr,
    // connector: Arc<Mutex<Box<dyn RunConnector>>>,
) -> Result<()> {
    debug!("udp? {:?}", addr);
    let udp_socket_base_res = UdpSocket::bind("127.0.0.1:0").await;
    if udp_socket_base_res.is_err() {
        acc.post_handshake(r.as_mut(), w.as_mut(), true, 0).await?;
        return Err(udp_socket_base_res.err().unwrap());
    }
    let udp_socket_base = udp_socket_base_res?;
    let udp_port = udp_socket_base.local_addr()?.port();
    acc.post_handshake(r.as_mut(), w.as_mut(), false, udp_port)
        .await?;
    let udp_socket_base = Arc::new(udp_socket_base);
    debug!("provide {} for {:?}", &udp_port, &addr);

    let (reader_interrupter, mut reader_interrupt_receiver) =
        oneshot::channel();
    let (reader_interrupter2, mut reader_interrupt_receiver2) =
        oneshot::channel();
    let (writer_interrupter, mut writer_interrupt_receiver) =
        oneshot::channel();
    let a: tokio::task::JoinHandle<Result<()>> = spawn(async move {
        let mut buf = [0u8; 1];
        loop {
            match r.read_exact(&mut buf).await {
                Ok(0) => {
                    warn!("udp tcp read 0，remote closed");
                    break;
                }
                Err(e) => {
                    debug!("udp tcp read error {:?}", e);
                    break;
                }
                Ok(_) => {} // Ok(n)
            }
        }
        let _ = reader_interrupter.send(());
        debug!("udp tcp done");
        Ok(())
    });

    let (udp_tunnel_sender, udp_tunnel_receiver) = oneshot::channel();
    let mut udp_tunnel_sender = Some(udp_tunnel_sender);
    debug!("udp loop start");
    let udp_socket = Arc::clone(&udp_socket_base);
    let config_clone_for_b = Arc::clone(&config);
    let router_clone_for_b = Arc::clone(&router);
    // let connector_clone_for_b = Arc::clone(&connector);

    let b: tokio::task::JoinHandle<Result<()>> = spawn(async move {
        let mut buf = [0u8; 65536];
        let mut udp_tunnel: Option<Arc<Box<dyn RunUdpStream>>> = None;
        loop {
            let interrupt_receiver = &mut reader_interrupt_receiver;
            let interrupt_receiver2 = &mut reader_interrupt_receiver2;
            let res: Result<(usize, SocketAddr)> = select! {
                _= interrupt_receiver=>{
                    Err(Error::new(ErrorKind::Interrupted, "interrupted"))
                },
                _= interrupt_receiver2=>{
                    Err(Error::new(ErrorKind::Interrupted, "interrupted2"))
                },
                n=udp_socket.recv_from(&mut buf) => {
                   Ok(n?)
                }
            };
            if res.is_err() {
                debug!("udp loop b udp server recv error {:?}", res.err());
                break;
            }
            let (n, src_addr) = res?;
            debug!(
                "udp server read src_addr {:?} {} {:?}",
                src_addr,
                n,
                &buf[..n]
            );
            let udp_packet = UDPPacket::parse(&buf[..n], src_addr)?;
            if (&udp_packet).data.is_empty() {
                warn!("udp drop");
                continue;
            }
            if udp_tunnel.is_none() {
                let client_name = router_clone_for_b
                    .route(
                        config_clone_for_b.listener.name.as_str(),
                        config_clone_for_b.listener.router.as_str(),
                        &RunAddr {
                            addr: (&udp_packet).meta.dst_addr.clone(),
                            port: (&udp_packet).meta.dst_port,
                            udp: false,
                            cache: None,
                        },
                    )
                    .await;
                let conn_conf =
                    config_clone_for_b.connector.get(client_name.as_str()).unwrap();
                let ctor = connector::create(conn_conf).await?;
                let t = Arc::new(
                    ctor.udp_tunnel(format!(
                        "{}:{}",
                        (&udp_packet).meta.dst_addr,
                        (&udp_packet).meta.dst_port,
                    ))
                    .await?
                    .unwrap(),
                );
                let _ =
                    udp_tunnel_sender.take().unwrap().send(Arc::clone(&t));
                udp_tunnel.replace(Arc::clone(&t));
            }

            debug!("udp server get udp_packet {:?}", &udp_packet);
            let udp_tunnel_ref = udp_tunnel.as_ref().unwrap();
            let res = udp_tunnel_ref.write(udp_packet).await;
            if res.is_err() {
                warn!("udp loop b udp tunnel write error {:?}", res.err());
                break;
            }
        }
        debug!("udp loop b done");
        let res = writer_interrupter.send(());
        if res.is_err() {
            debug!("udp loop b interrupter error {:?}", res.err());
        }
        Ok(())
    });
    let udp_socket_c = Arc::clone(&udp_socket_base);
    let c: tokio::task::JoinHandle<Result<()>> = spawn(async move {
        let udp_tunnel_res = udp_tunnel_receiver.await;
        if udp_tunnel_res.is_err(){
            debug!("udp_tunnel_receiver error {:?}", udp_tunnel_res.err());
            let _ = reader_interrupter2.send(());
            return Ok(());
        }
        let udp_tunnel = udp_tunnel_res.unwrap();
        'c_job: loop {
            let interrupt_receiver = &mut writer_interrupt_receiver;
            let res: Result<UDPPacket> = select! {
                _= interrupt_receiver=>{
                    Err(Error::new(ErrorKind::Interrupted, "interrupted"))
                },
                n=udp_tunnel.read() => {
                   Ok(n?)
                }
            };
            if res.is_err() {
                debug!("udp loop c tunnel read error {:?}", res.err());
                break;
            }
            let udp_packet = res?;
            let (payloads, src_addr_str, _) = udp_packet.reply_bytes();
            debug!(
                "udp tunnel udp_packet read src {} {:?} \n{:?}",
                &src_addr_str, udp_packet, &payloads
            );
            for payload in payloads {
                let res = udp_socket_c
                    .send_to(payload.as_slice(), src_addr_str.clone())
                    .await;
                if res.is_err() {
                    warn!(
                        "udp loop c udp server send error {:?}",
                        res.err()
                    );
                    break 'c_job;
                }
            }
        }
        debug!("udp loop c done");
        let _ = reader_interrupter2.send(());
        Ok(())
    });
    let _ = a.await;
    let _ = b.await;
    let _ = c.await;
    debug!("udp loop done");
    Ok(())
}
