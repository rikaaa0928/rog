use crate::def::{RunAccStream, RunAcceptor, RunListener, RunStream};
use crate::object::config::ObjectConfig;
use crate::proto::v1::pb::rog_reverse_service_client::RogReverseServiceClient;
use crate::proto::v1::pb::{ManagerReq, ManagerRes, RevStreamReq, RevUdpReq, RevUdpRes};
use crate::stream::rev_grpc_client::RevGrpcClientRunStream;
use crate::util::RunAddr;
use futures::StreamExt;
use log::{debug, error, info, warn};
use std::io;
use std::io::Error;
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::mpsc::{Receiver, Sender};
use tokio::sync::{Mutex, mpsc};
use tokio::time::sleep;
use tokio::{select, spawn};
use tonic::codegen::tokio_stream;
use tonic::transport::{Channel, Endpoint};
use tonic::{Request, Status, Streaming};

async fn handle_manage_req(
    req: ManagerRes,
    client: &mut RogReverseServiceClient<Channel>,
    mtx: &mpsc::Sender<ManagerReq>,
    tx: &mpsc::Sender<RevGrpcClientRunStream>,
    auth: &str,
    tag: &str,
) -> Result<(), ()> {
    if req.udp.unwrap() == 0 {
        // tcp
        let (stx, srx) = mpsc::channel::<RevStreamReq>(8);
        let srx = tokio_stream::wrappers::ReceiverStream::new(srx);
        let srx = Request::new(srx);
        match client.stream(srx).await {
            Ok(stream) => {
                let uuid_str = uuid::Uuid::new_v4().to_string();
                if let Err(e) = mtx
                    .send(ManagerReq {
                        auth: auth.to_string(),
                        tag: tag.to_string(),
                        conn_id: Some(uuid_str.clone()),
                        addr_info: req.addr_info.clone(),
                        udp: Some(0),
                    })
                    .await
                {
                    error!("rev grpc manager send error {}", e);
                    return Err(());
                }
                if let Err(e) = stx
                    .send(RevStreamReq {
                        auth: auth.to_string(),
                        payload: None,
                        conn_id: Some(uuid_str),
                    })
                    .await
                {
                    error!("rev grpc stream send error {}", e);
                    return Err(());
                }

                let stream = stream.into_inner();
                let mut new_stream = RevGrpcClientRunStream::new(Arc::new(Mutex::new(stream)), stx);
                let addr_info = req.addr_info.clone().unwrap();
                new_stream.set_info(&mut |x| {
                    x.protocol_name = "rev_grpc".to_owned();
                    x.src_addr = Some(addr_info.src_addr.clone());
                    x.src_port = Some(addr_info.src_port as u16);
                    x.dst_addr = Some(addr_info.dst_addr.clone());
                    x.dst_port = Some(addr_info.dst_port as u16);
                    x.udp = Some(false);
                });
                if let Err(_) = tx.send(new_stream).await {
                    error!("rev grpc manager stream send error");
                    return Err(());
                }
            }
            Err(e) => {
                error!("rev grpc manager stream send error {}", e);
                return Err(());
            }
        }
    } else {
        // udp
        let (utx, urx) = mpsc::channel::<RevUdpReq>(8);
        let urx = tokio_stream::wrappers::ReceiverStream::new(urx);
        let urx = Request::new(urx);
        match client.udp(urx).await {
            Ok(_stream) => {
                // todo: udp
            }
            Err(e) => {
                error!("rev grpc udp stream send error {}", e);
                return Err(());
            }
        }
    }
    Ok(())
}

pub struct RevGrpcListener {
    cfg: ObjectConfig,
}

impl RevGrpcListener {
    pub fn new(cfg: ObjectConfig) -> RevGrpcListener {
        RevGrpcListener { cfg }
    }
}

pub struct RevGrpcRunListener {
    receiver: Arc<Mutex<Receiver<RevGrpcClientRunStream>>>,
    udp_receiver: Arc<
        Mutex<
            Receiver<(
                Streaming<RevUdpReq>,
                Sender<Result<RevUdpRes, Status>>,
                String,
            )>,
        >,
    >,
    auth: String,
}

#[async_trait::async_trait]
impl RunListener for RevGrpcListener {
    async fn listen(&self, addr: &str) -> std::io::Result<Box<dyn RunAcceptor>> {
        let endpoint = Endpoint::new(addr.to_string())
            .map_err(|e| io::Error::other("rev grpc endpoint new error"))?;

        let (tx, rx) = mpsc::channel(8);
        let (utx, urx) = mpsc::channel(8);
        let auth = self.cfg.listener.pw.clone().unwrap();
        let tag = self.cfg.listener.name.clone();
        spawn(async move {
            loop {
                let channel;
                loop {
                    let t = endpoint.connect().await;
                    match t {
                        Ok(c) => {
                            channel = c;
                            break;
                        }
                        Err(_) => {
                            debug!("rev grpc endpoint connect error");
                            sleep(Duration::from_millis(300)).await;
                        }
                    }
                }
                let (mtx, mrx) = mpsc::channel::<ManagerReq>(8);
                let mrx = tokio_stream::wrappers::ReceiverStream::new(mrx);
                let mrx = Request::new(mrx);

                let mut client = RogReverseServiceClient::new(channel);

                let manager_stream = client.manager(mrx).await;
                match manager_stream {
                    Ok(manager_stream) => {
                        match mtx
                            .send(ManagerReq {
                                auth: auth.clone(),
                                tag: tag.clone(),
                                addr_info: None,
                                udp: None,
                                conn_id: None,
                            })
                            .await
                        {
                            Ok(_) => {}
                            Err(e) => {
                                error!("manager stream send error {}", e);
                                continue;
                            }
                        }
                        let mut manager_stream = manager_stream.into_inner();
                        loop {
                            select! {
                                manage_req = manager_stream.next() => {
                                    match manage_req {
                                        None => {
                                            info!("manager stream next none");
                                            break;
                                        }
                                        Some(Ok(req)) => {
                                            // todo: param check
                                            if handle_manage_req(req, &mut client, &mtx, &tx, &auth, &tag).await.is_err() {
                                                break;
                                            }
                                        }
                                        Some(Err(err)) => {
                                            warn!("manager stream error: {:?}", err);
                                            break;
                                        }
                                    }
                                }
                            }
                        }
                    }
                    Err(_) => {
                        info!("rev grpc manager error");
                        sleep(Duration::from_millis(300)).await;
                    }
                }
            }
        });

        Ok(Box::new(RevGrpcRunListener {
            receiver: Arc::new(Mutex::new(rx)),
            udp_receiver: Arc::new(Mutex::new(urx)),
            auth: self.cfg.listener.pw.clone().unwrap(),
        }))
    }
}

#[async_trait::async_trait]
impl RunAcceptor for RevGrpcRunListener {
    async fn accept(&self) -> std::io::Result<(RunAccStream, SocketAddr)> {
        let mut receiver = self.receiver.lock().await;
        // let mut udp_receiver = self.udp_receiver.lock().await;
        select! {
            // todo: udp
            // r = udp_receiver.recv() => {
            //     match r {
            //         None => Err(Error::other("receiver closed")),
            //         Some((r,w,a)) => Ok(((RunAccStream::UDPSocket(
            //             (
            //                 Box::new(GrpcUdpServerReadHalf::new(r,a)),
            //                 Box::new(GrpcUdpServerWriteHalf::new(w))
            //                 )
            //         )), "127.0.9.28:2809".parse().unwrap())),
            //     }
            // }
            r = receiver.recv() => {
                match r {
                    None => Err(Error::other("receiver closed")),
                    Some(stream) => Ok((RunAccStream::TCPStream( Box::new(stream)), "127.0.0.1:2809".parse().unwrap())),
                }
            }
        }
    }

    async fn handshake(
        &self,
        stream: &mut dyn RunStream,
    ) -> std::io::Result<(RunAddr, Option<Vec<u8>>)> {
        let info = stream.get_info();
        Ok((
            RunAddr {
                addr: info.dst_addr.clone().unwrap(),
                port: info.dst_port.unwrap(),
                udp: info.udp.unwrap(),
            },
            None,
        ))
    }
}
