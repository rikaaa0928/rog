use std::io::{Error, ErrorKind, Result};
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::sync::{Arc};
use std::time::Duration;
use log::debug;
use tokio::{select, spawn};
use tokio::net::UdpSocket;
use tokio::sync::{oneshot, Mutex};
use tokio::time::sleep;
use crate::connector::tcp::TcpRunConnector;
use crate::def::{RunAcceptor, RunConnector, RunListener, RunReadHalf, RunStream, RunUdpConnector, RunWriteHalf, UDPPacket};
use crate::listener::socks5::SocksRunAcceptor;
use crate::listener::tcp::TcpRunListener;
use crate::util;
use crate::util::socks5::UDP_ERROR_STR;

#[tokio::test]
async fn test_tcp() -> Result<()> {
    spawn(async {
        let listener = TcpRunListener::listen("127.0.0.1:12345".to_string()).await?;
        println!("server listen");
        let (stream, addr) = listener.accept().await?;
        println!("Accepted connection from {}", addr);
        let _ = spawn(async move {
            let (mut r, mut w) = stream.split();
            listener.handshake(&mut r, &mut w).await?;
            listener.post_handshake(&mut r, &mut w, false).await?;
            let a = spawn(async move {
                let mut buf = vec![0u8; 10];
                let n = r.read(&mut buf).await?;
                println!("Read {} bytes: {:?}", n, &&buf[..n]);
                Ok::<(), Error>(())
            });
            let b = spawn(async move {
                let _ = w.write("abcd".to_string().as_bytes()).await?;
                Ok::<(), Error>(())
            });
            a.await?;
            b.await?;
            Ok::<(), Error>(())
        });
        sleep(Duration::from_secs(1)).await;
        Ok::<(), Error>(())
    });
    sleep(Duration::from_secs(1)).await;
    let stream = TcpRunConnector::new().connect("127.0.0.1:12345".to_string()).await?;
    let (mut r, mut w) = stream.split();
    let a = spawn(async move {
        let mut buf = vec![0u8; 10];
        let n = r.read(&mut buf).await?;
        println!("Read {} bytes: {:?}", n, &&buf[..n]);
        Ok::<(), Error>(())
    });
    let b = spawn(async move {
        let _ = w.write("abcd".to_string().as_bytes()).await?;
        Ok::<(), Error>(())
    });
    let _ = a.await?;
    let _ = b.await?;
    Ok(())
}

#[tokio::test]
async fn test_socks5() -> Result<()> {
    let listener = TcpRunListener::listen("127.0.0.1:12345".to_string()).await?;
    let socks5 = Arc::new(SocksRunAcceptor::new(listener, None, None));
    let connector = Arc::new(Mutex::new(TcpRunConnector::new()));
    loop {
        let connector = Arc::clone(&connector);
        let socks5 = Arc::clone(&socks5);
        let (s, a) = socks5.accept().await?;
        println!("Accepted connection from {}", a);
        spawn(async move {
            let (mut r, mut w) = s.split();
            // let udp_tunnel=Arc::new(&connector).lock().await.udp_tunnel();
            let addr_res = socks5.handshake(&mut r, &mut w).await;
            match addr_res {
                Err(e) => {
                    println!("Handshake error: {}", e);
                }
                Ok(addr) => {
                    let addr_ref = &addr;
                    if addr_ref.udp {
                        println!("udp? {:?}", addr_ref);
                        let mut udp_socket_base = UdpSocket::bind("127.0.0.1:0").await?;
                        let udp_port = udp_socket_base.local_addr()?.port();
                        let udp_socket = Arc::new(udp_socket_base);
                        let confirm = util::socks5::confirm::Confirm::new(false, udp_port);
                        w.write(&confirm.to_bytes()).await?;
                        let (reader_interrupter, mut reader_interrupt_receiver) = oneshot::channel();
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
                            Ok(())
                        });
                        let l_addr: std::result::Result<SocketAddr, _> = "127.0.0.1:54321".parse();
                        if l_addr.is_err() {
                            return Err(Error::new(ErrorKind::InvalidInput, l_addr.unwrap_err()));
                        }
                        let udp_tunnel = connector.udp_tunnel(l_addr.unwrap()).await?;
                        if udp_tunnel.is_none() {
                            println!("udp tunnel none");
                            return Ok(());
                        }
                        let udp_tunnel = udp_tunnel.unwrap();
                        //loop
                        let b: tokio::task::JoinHandle<Result<()>> = spawn(async move {
                            let mut buf = [0u8; 65536];
                            loop {
                                let interrupt_receiver = &mut reader_interrupt_receiver;
                                let res: Result<(usize, SocketAddr)> = select! {
                                    _= interrupt_receiver=>{
                                        Err(Error::new(ErrorKind::Interrupted, "interrupted"))?
                                    },
                                    n=udp_socket.recv_from(&mut buf) => {
                                       Ok(n?)
                                    }

                                };
                                if res.is_err() {
                                    break;
                                }
                                let start: usize = 0;
                                let (n, src_addr) = res?;
                                let udp_packet = UDPPacket::parse(&buf[..n], src_addr)?;
                            }
                            Ok(())
                        });
                        let _ = a.await;
                        let _ = b.await;
                        println!("udp done");
                        return Ok(());
                    }
                    println!("Handshake successful {:?}", addr_ref);
                    let client_stream_res = Arc::clone(&connector).lock().await.connect(addr_ref.endpoint()).await;
                    let mut error = false;
                    if client_stream_res.is_err() {
                        error = true;
                    }
                    let client_stream = client_stream_res?;
                    socks5.post_handshake(&mut r, &mut w, error).await?;
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
    Ok(())
}