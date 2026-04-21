use crate::def::{RunAccStream, RunAcceptor, RunListener, RunStream};
use crate::object::config::ObjectConfig;
use crate::stream::pb_tcp_server::PbTcpServerRunStream;
use crate::stream::pb_tcp_udp_server::{PbTcpUdpServerReader, PbTcpUdpServerWriter};
use crate::util::RunAddr;
use crate::util::tcp_frame::*;
use log::{error, warn};
use std::io::Error;
use std::net::SocketAddr;
use std::sync::Arc;
use tokio::net::TcpListener;
use tokio::net::tcp::{OwnedReadHalf, OwnedWriteHalf};
use tokio::sync::mpsc::Receiver;
use tokio::sync::{Mutex, mpsc};
use tokio::{select, spawn};

pub struct PbTcpListener {
    cfg: ObjectConfig,
}

impl PbTcpListener {
    pub fn new(cfg: ObjectConfig) -> PbTcpListener {
        PbTcpListener { cfg }
    }
}

pub struct PbTcpRunAcceptor {
    stream_receiver: Arc<Mutex<Receiver<PbTcpServerRunStream>>>,
    udp_receiver: Arc<Mutex<Receiver<(OwnedReadHalf, OwnedWriteHalf, String)>>>,
    auth: String,
}

#[async_trait::async_trait]
impl RunListener for PbTcpListener {
    async fn listen(&self, addr: &str) -> std::io::Result<Box<dyn RunAcceptor>> {
        let (stream_tx, stream_rx) = mpsc::channel::<PbTcpServerRunStream>(8);
        let (udp_tx, udp_rx) = mpsc::channel::<(OwnedReadHalf, OwnedWriteHalf, String)>(8);
        let auth = self.cfg.listener.pw.as_ref().unwrap().clone();
        let bind_addr = addr.to_owned();

        spawn(async move {
            let listener = match TcpListener::bind(&bind_addr).await {
                Ok(l) => l,
                Err(e) => {
                    error!("pb_tcp listener bind error: {}", e);
                    return;
                }
            };

            loop {
                let (tcp_stream, addr) = match listener.accept().await {
                    Ok(s) => s,
                    Err(e) => {
                        error!("pb_tcp listener accept error: {}", e);
                        continue;
                    }
                };
                let stream_tx = stream_tx.clone();
                let udp_tx = udp_tx.clone();
                let auth = auth.clone();
                spawn(async move {
                    let (mut reader, writer) = tcp_stream.into_split();
                    let conn_type = match read_conn_type(&mut reader).await {
                        Ok(t) => t,
                        Err(e) => {
                            warn!("pb_tcp read conn type from {} error: {}", addr, e);
                            return;
                        }
                    };
                    match conn_type {
                        CONN_TYPE_STREAM => {
                            let stream = PbTcpServerRunStream::new(reader, writer);
                            if stream_tx.send(stream).await.is_err() {
                                warn!("pb_tcp stream channel closed");
                            }
                        }
                        CONN_TYPE_UDP => {
                            if udp_tx.send((reader, writer, auth)).await.is_err() {
                                warn!("pb_tcp udp channel closed");
                            }
                        }
                        _ => {
                            warn!("pb_tcp unknown conn type {} from {}", conn_type, addr);
                        }
                    }
                });
            }
        });

        Ok(Box::new(PbTcpRunAcceptor {
            stream_receiver: Arc::new(Mutex::new(stream_rx)),
            udp_receiver: Arc::new(Mutex::new(udp_rx)),
            auth: self.cfg.listener.pw.as_ref().unwrap().clone(),
        }))
    }
}

#[async_trait::async_trait]
impl RunAcceptor for PbTcpRunAcceptor {
    async fn accept(&self) -> std::io::Result<(RunAccStream, SocketAddr)> {
        let mut stream_receiver = self.stream_receiver.lock().await;
        let mut udp_receiver = self.udp_receiver.lock().await;
        select! {
            r = udp_receiver.recv() => {
                match r {
                    None => Err(Error::other("udp receiver closed")),
                    Some((reader, writer, pw)) => {
                        let reader = Arc::new(Mutex::new(reader));
                        let writer = Arc::new(Mutex::new(writer));
                        Ok((
                            RunAccStream::UDPSocket((
                                Box::new(PbTcpUdpServerReader::new(
                                    Arc::clone(&reader),
                                    pw.clone(),
                                    pw.clone(),
                                )),
                                Box::new(PbTcpUdpServerWriter::new(
                                    Arc::clone(&writer),
                                    pw,
                                )),
                            )),
                            "127.0.9.28:2809".parse().unwrap(),
                        ))
                    }
                }
            }
            r = stream_receiver.recv() => {
                match r {
                    None => Err(Error::other("stream receiver closed")),
                    Some(stream) => Ok((
                        RunAccStream::TCPStream(Box::new(stream)),
                        "127.0.0.1:2809".parse().unwrap(),
                    )),
                }
            }
        }
    }

    async fn handshake(
        &self,
        stream: &mut dyn RunStream,
    ) -> std::io::Result<(RunAddr, Option<Vec<u8>>)> {
        stream.set_info(&mut |x| x.protocol_name = "pb_tcp".to_string());
        let stream = stream
            .as_any_mut()
            .downcast_mut::<PbTcpServerRunStream>()
            .unwrap();
        match stream.handshake(&self.auth).await? {
            Some((addr, auth)) => {
                if auth != self.auth {
                    return Err(Error::other("invalid auth"));
                }
                Ok((addr, None))
            }
            None => Err(Error::other("handshake failed")),
        }
    }
}
