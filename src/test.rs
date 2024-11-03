use std::io::{Error, ErrorKind, Result};
use std::net::{SocketAddr};
use std::sync::{Arc};
use tokio::{select, spawn};
use tokio::net::UdpSocket;
use tokio::sync::{oneshot, Mutex};
use crate::connector::tcp::TcpRunConnector;
use crate::def::{RunAcceptor, RunConnector, RunListener, RunReadHalf, RunStream, RunUdpStream, RunWriteHalf, UDPPacket};
use crate::listener::socks5::SocksRunAcceptor;
use crate::listener::tcp::TcpRunListener;

#[cfg(test)]
#[tokio::test]
async fn test_socks5() -> Result<()> {
    let listener = TcpRunListener::listen("127.0.0.1:12345").await?;
    let socks5 = Arc::new(SocksRunAcceptor::new(listener, None, None));
    let connector = Arc::new(Mutex::new(TcpRunConnector::new()));
    loop {
        let connector = Arc::clone(&connector);
        let socks5 = Arc::clone(&socks5);
        let (s, a) = socks5.accept().await?;
        println!("Accepted connection from {}", a);
        let job = spawn(async move {
            let (mut r, mut w) = s.split();
            // let udp_tunnel=Arc::new(&connector).lock().await.udp_tunnel();
            let addr_res = socks5.handshake(r.as_mut(), w.as_mut()).await;
            match addr_res {
                Err(e) => {
                    println!("Handshake error: {}", e);
                }
                Ok(addr) => {
                    let addr_ref = &addr;
                    if addr_ref.udp {
                        println!("udp? {:?}", addr_ref);
                        let udp_socket_base_res = UdpSocket::bind("127.0.0.1:0").await;
                        if udp_socket_base_res.is_err() {
                            socks5.post_handshake(r.as_mut(), w.as_mut(), true, 0).await?;
                            return Err(udp_socket_base_res.err().unwrap());
                        }
                        let udp_socket_base = udp_socket_base_res?;
                        let udp_port = udp_socket_base.local_addr()?.port();
                        socks5.post_handshake(r.as_mut(), w.as_mut(), false, udp_port).await?;
                        // let confirm = util::socks5::confirm::Confirm::new(false, udp_port);
                        // println!("post handshake {:?}", &confirm.to_bytes());
                        // w.write(&confirm.to_bytes()).await?;
                        let udp_socket_base = Arc::new(udp_socket_base);
                        println!("provide {} for {:?}", &udp_port, addr_ref);

                        let (reader_interrupter, mut reader_interrupt_receiver) = oneshot::channel();
                        let (reader_interrupter2, mut reader_interrupt_receiver2) = oneshot::channel();
                        let (writer_interrupter, mut writer_interrupt_receiver) = oneshot::channel();
                        let a: tokio::task::JoinHandle<Result<()>> = spawn(async move {
                            let mut buf = [0u8; 1];
                            loop {
                                let res = r.read(&mut buf).await;
                                if res.is_err() {
                                    println!("udp tcp read error {:?}", res.err());
                                    break;
                                }
                            }
                            let _ = reader_interrupter.send(());
                            println!("udp tcp done");
                            Ok(())
                        });

                        let udp_tunnel = connector.udp_tunnel(addr.endpoint()).await?;
                        if udp_tunnel.is_none() {
                            println!("udp tunnel none");
                            return Ok(());
                        }
                        let udp_tunnel_base = Arc::new(udp_tunnel.unwrap());
                        //loop
                        println!("udp loop start");
                        let udp_tunnel = Arc::clone(&udp_tunnel_base);
                        let udp_socket = Arc::clone(&udp_socket_base);
                        let b: tokio::task::JoinHandle<Result<()>> = spawn(async move {
                            let mut buf = [0u8; 65536];
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
                                    println!("udp loop b udp server recv error {:?}", res.err());
                                    break;
                                }
                                let (n, src_addr) = res?;
                                println!("udp server read src_addr {:?} {} {:?}", src_addr, n, &buf[..n]);
                                let udp_packet = UDPPacket::parse(&buf[..n], src_addr)?;
                                if (&udp_packet).data.is_empty() {
                                    println!("udp drop");
                                    continue;
                                }
                                println!("udp server get udp_packet {:?}", &udp_packet);
                                let res = udp_tunnel.write(udp_packet).await;
                                if res.is_err() {
                                    println!("udp loop b udp tunnel write error {:?}", res.err());
                                    break;
                                }
                            }
                            println!("udp loop b done");
                            let res = writer_interrupter.send(());
                            if res.is_err() {
                                println!("udp loop b interrupter error {:?}", res.err());
                            }
                            Ok(())
                        });
                        let udp_tunnel = Arc::clone(&udp_tunnel_base);
                        let udp_socket = Arc::clone(&udp_socket_base);
                        let c: tokio::task::JoinHandle<Result<()>> = spawn(async move {
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
                                    println!("udp loop c tunnel read error {:?}", res.err());
                                    break;
                                }
                                let udp_packet = res?;
                                let (payloads, src_addr_str, _) = udp_packet.bytes();
                                println!("udp tunnel udp_packet read src {} {:?} \n{:?}", &src_addr_str, udp_packet, &payloads);
                                for payload in payloads {
                                    let res = udp_socket.send_to(payload.as_slice(), src_addr_str.clone()).await;
                                    if res.is_err() {
                                        println!("udp loop c udp server send error {:?}", res.err());
                                        break 'c_job;
                                    }
                                }
                            }
                            println!("udp loop c done");
                            let _ = reader_interrupter2.send(());
                            Ok(())
                        });
                        let _ = a.await;
                        let _ = b.await;
                        let _ = c.await;
                        println!("udp loop done");
                        return Ok(());
                    }
                    println!("Handshake successful {:?}", addr_ref);
                    let client_stream_res = Arc::clone(&connector).lock().await.connect(addr_ref.endpoint()).await;
                    let mut error = false;
                    if client_stream_res.is_err() {
                        error = true;
                    }
                    let client_stream = client_stream_res?;
                    socks5.post_handshake(r.as_mut(), w.as_mut(), error, 0).await?;
                    let (mut tcp_r, mut tcp_w) = client_stream.split();
                    let (reader_interrupter, mut reader_interrupt_receiver) = oneshot::channel();
                    let (writer_interrupter, mut writer_interrupt_receiver) = oneshot::channel();
                    println!("start loop");
                    let x = spawn(async move {
                        let mut buf = [0u8; 65536];
                        loop {
                            let reader_interrupt_receiver = &mut reader_interrupt_receiver;
                            let n_res = select! {
                                tn=r.read(&mut buf) => tn,
                               _=reader_interrupt_receiver=>Err(Error::new(ErrorKind::Other, "Interrupted")),
                            };
                            if n_res.is_err() {
                                break;
                            }
                            let n = n_res.unwrap();
                            let res = tcp_w.write(&buf[..n]).await;
                            if res.is_err() {
                                break;
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
                            let res = w.write(&buf[..n]).await;
                            if res.is_err() {
                                break;
                            }
                        }
                        let _ = reader_interrupter.send(());
                    });
                    let _ = x.await;
                    let _ = y.await;
                    println!("end loop");
                }
            }
            Ok::<(), Error>(())
        });
    }
}