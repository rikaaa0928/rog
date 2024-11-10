use crate::def::{Router, RunConnector, RunUdpStream, UDPPacket};
use crate::object::config::ObjectConfig;
use crate::router::DefaultRouter;
use crate::util::RunAddr;
use crate::{connector, listener};
use std::io;
use std::io::{Error, ErrorKind, Result};
use std::net::SocketAddr;
use std::sync::Arc;
use log::{debug, error, warn};
use tokio::net::UdpSocket;
use tokio::sync::{oneshot, Mutex};
use tokio::{select, spawn};

pub mod config;

pub struct Object {
    config: ObjectConfig,
}

impl Object {
    pub fn new(config: ObjectConfig) -> Self {
        Self { config }
    }

    pub async fn start(&self) -> io::Result<()> {
        let config = self.config.clone();
        let acc = listener::create(&config).await?;
        let acc = Arc::new(acc);
        let router = Arc::new(DefaultRouter::new(&config.router));
        loop {
            let (s, _) = acc.accept().await?;
            let acc = Arc::clone(&acc);
            let router = Arc::clone(&router);
            let config = config.clone();
            spawn(async move {
                let (mut r, mut w) = s.split();
                // let udp_tunnel=Arc::new(&connector).lock().await.udp_tunnel();
                let addr_res = acc.handshake(r.as_mut(), w.as_mut()).await;
                match addr_res {
                    Err(e) => {
                        error!("Handshake error: {}", e);
                    }
                    Ok(addr) => {
                        let addr_ref = &addr;
                        let client_name = router.route(addr_ref).await?;
                        let conn_conf = config.connector.get(client_name.as_str()).unwrap();
                        let connector = Arc::new(Mutex::new(connector::create(conn_conf).await?));
                        if addr_ref.udp {
                            debug!("udp? {:?}", addr_ref);
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
                            debug!("provide {} for {:?}", &udp_port, addr_ref);

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
                                        Ok(n) => {}
                                    }
                                }
                                let _ = reader_interrupter.send(());
                                debug!("udp tcp done");
                                Ok(())
                            });

                            // let udp_tunnel = connector.lock().await.udp_tunnel(addr.endpoint()).await?;
                            // if udp_tunnel.is_none() {
                            //     println!("udp tunnel none");
                            //     return Ok(());
                            // }
                            // let udp_tunnel_base = Arc::new(udp_tunnel.unwrap());
                            //loop
                            let (udp_tunnel_sender, mut udp_tunnel_receiver) = oneshot::channel();
                            let mut udp_tunnel_sender = Some(udp_tunnel_sender);
                            debug!("udp loop start");
                            // let udp_tunnel_base: Arc<Mutex<Option<Box<dyn RunUdpStream>>>> = Arc::new(Mutex::new(None));
                            let udp_socket = Arc::clone(&udp_socket_base);
                            // let udp_tunnel_r = Arc::clone(&udp_tunnel_base);
                            let b: tokio::task::JoinHandle<Result<()>> = spawn(async move {
                                let mut buf = [0u8; 65536];
                                let mut udp_tunnel: Option<Arc<Box<dyn RunUdpStream>>> = None;
                                // let mut udp_tunnel = udp_tunnel.as_mut();
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
                                        debug!(
                                            "udp loop b udp server recv error {:?}",
                                            res.err()
                                        );
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
                                        let client_name = router
                                            .route(&RunAddr {
                                                addr: (&udp_packet).meta.dst_addr.clone(),
                                                port: (&udp_packet).meta.dst_port,
                                                udp: false,
                                            })
                                            .await?;
                                        let conn_conf =
                                            config.connector.get(client_name.as_str()).unwrap();
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
                                    let udp_tunnel = udp_tunnel.as_mut().unwrap();
                                    let res = udp_tunnel.write(udp_packet).await;
                                    if res.is_err() {
                                        warn!(
                                            "udp loop b udp tunnel write error {:?}",
                                            res.err()
                                        );
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
                            // let udp_tunnel = Arc::clone(&udp_tunnel_base);
                            let udp_socket = Arc::clone(&udp_socket_base);
                            let c: tokio::task::JoinHandle<Result<()>> = spawn(async move {
                                let udp_tunnel_res = udp_tunnel_receiver.await;
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
                                        let res = udp_socket
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
                            return Ok(());
                        }
                        debug!("Handshake successful {:?}", addr_ref);
                        let client_stream_res = Arc::clone(&connector)
                            .lock()
                            .await
                            .connect(addr_ref.endpoint())
                            .await;
                        let mut error = false;
                        if client_stream_res.is_err() {
                            error = true;
                        }
                        let client_stream = client_stream_res?;
                        acc.post_handshake(r.as_mut(), w.as_mut(), error, 0).await?;
                        let (mut tcp_r, mut tcp_w) = client_stream.split();
                        let (reader_interrupter, mut reader_interrupt_receiver) =
                            oneshot::channel();
                        let (writer_interrupter, mut writer_interrupt_receiver) =
                            oneshot::channel();
                        debug!("start loop");
                        let x = spawn(async move {
                            let mut buf = [0u8; 65536];
                            loop {
                                let reader_interrupt_receiver = &mut reader_interrupt_receiver;
                                match select! {
                                    tn=r.read(&mut buf) => tn,
                                   _=reader_interrupt_receiver=>Err(Error::new(ErrorKind::Other, "Interrupted")),
                                } {
                                    Err(e) => {
                                        break;
                                    }
                                    Ok(0) => {
                                        break;
                                    }
                                    Ok(n) => {
                                        let res = tcp_w.write(&buf[..n]).await;
                                        if res.is_err() {
                                            break;
                                        }
                                    }
                                }
                            }
                            let _ = writer_interrupter.send(());
                        });
                        let y = spawn(async move {
                            let mut buf = [0u8; 65536];
                            loop {
                                let writer_interrupt_receiver = &mut writer_interrupt_receiver;
                                let n_res = select! {
                                    tn=tcp_r.read(&mut buf) => tn,
                                   _=writer_interrupt_receiver=>Err(Error::new(ErrorKind::Other, "Interrupted")),
                                };
                                if n_res.is_err() {
                                    break;
                                }
                                let n = n_res.unwrap();
                                if n == 0 {
                                    break;
                                }
                                let res = w.write(&buf[..n]).await;
                                if res.is_err() {
                                    break;
                                }
                            }
                            let _ = reader_interrupter.send(());
                        });
                        let _ = x.await;
                        let _ = y.await;
                        debug!("end loop");
                    }
                }
                Ok::<(), Error>(())
            });
        }
        Ok(())
    }
}
