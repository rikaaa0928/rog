use crate::def::config::get_option_bool;
use crate::def::{RunAccStream, RunAcceptor, RunListener, RunStream, RunUdpReader, RunUdpWriter};
use crate::object::config::ObjectConfig;
use crate::proto::v1::pb::rog_reverse_service_client::RogReverseServiceClient;
use crate::proto::v1::pb::{ManagerReq, ManagerRes, RevStreamReq, RevUdpReq};
use crate::stream::rev_grpc_client::RevGrpcClientRunStream;
use crate::stream::rev_grpc_udp_client::{RevGrpcUdpClientReader, RevGrpcUdpClientWriter};
use crate::util::RunAddr;
use crate::util::grpc_transport::connect_channel_without_proxy;
use futures::StreamExt;
use log::{debug, error, info, trace, warn};
use std::io;
use std::io::Error;
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::mpsc::Receiver;
use tokio::sync::{Mutex, mpsc};
use tokio::time::sleep;
use tokio::{select, spawn};
use tonic::Request;
use tonic::codegen::tokio_stream;
use tonic::transport::{Channel, Endpoint};

async fn handle_manage_req(
    req: ManagerRes,
    client: &mut RogReverseServiceClient<Channel>,
    tx: &mpsc::Sender<RevGrpcClientRunStream>,
    udp_tx: &mpsc::Sender<(Box<dyn RunUdpReader>, Box<dyn RunUdpWriter>)>,
    auth: &str,
) -> Result<(), ()> {
    if req.udp.unwrap() == 0 {
        // tcp
        let (stx, srx) = mpsc::channel::<RevStreamReq>(8);
        let srx = tokio_stream::wrappers::ReceiverStream::new(srx);
        let srx = Request::new(srx);

        let uuid_str = req.conn_id.clone().unwrap_or_default();
        if let Err(e) = stx
            .send(RevStreamReq {
                auth: auth.to_string(),
                payload: None,
                conn_id: Some(uuid_str),
            })
            .await
        {
            error!("rev grpc stream initial auth send error {}", e);
            return Ok(());
        }

        match client.stream(srx).await {
            Ok(stream) => {
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
                error!("rev grpc manager stream accept error {}", e);
                return Ok(());
            }
        }
    } else {
        // udp
        let conn_id = req.conn_id.clone().unwrap_or_default();
        let (utx, urx) = mpsc::channel::<RevUdpReq>(8);
        let urx = tokio_stream::wrappers::ReceiverStream::new(urx);
        let urx = Request::new(urx);

        // 先发送握手包（conn_id + auth）
        if let Err(e) = utx
            .send(RevUdpReq {
                auth: auth.to_string(),
                payload: None,
                addr_info: None,
                conn_id: Some(conn_id),
            })
            .await
        {
            error!("rev grpc udp initial auth send error {}", e);
            return Ok(());
        }

        match client.udp(urx).await {
            Ok(stream) => {
                let res_stream = stream.into_inner();
                let reader =
                    Box::new(RevGrpcUdpClientReader::new(res_stream)) as Box<dyn RunUdpReader>;
                let writer = Box::new(RevGrpcUdpClientWriter::new(utx, auth.to_string()))
                    as Box<dyn RunUdpWriter>;
                if let Err(_) = udp_tx.send((reader, writer)).await {
                    error!("rev grpc udp pair send error");
                    return Err(());
                }
            }
            Err(e) => {
                error!("rev grpc udp stream open error {}", e);
                return Ok(());
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
    udp_receiver: Arc<Mutex<Receiver<(Box<dyn RunUdpReader>, Box<dyn RunUdpWriter>)>>>,
}

#[async_trait::async_trait]
impl RunListener for RevGrpcListener {
    async fn listen(&self, addr: &str) -> std::io::Result<Box<dyn RunAcceptor>> {
        let keep_alive = get_option_bool(&self.cfg.listener.options, "keep_alive");
        let endpoint = Endpoint::new(addr.to_string())
            .map_err(|e| io::Error::other("rev grpc endpoint new error"))?;
        let endpoint = if keep_alive {
            endpoint
                .http2_keep_alive_interval(Duration::from_secs(30))
                .keep_alive_timeout(Duration::from_secs(10))
                .keep_alive_while_idle(true)
        } else {
            endpoint
        };

        let (tx, rx) = mpsc::channel(8);
        let (utx, urx) = mpsc::channel(8);
        let auth = self.cfg.listener.pw.clone().unwrap();
        let tag = self.cfg.listener.name.clone();
        spawn(async move {
            loop {
                let channel;
                loop {
                    let t = connect_channel_without_proxy(endpoint.clone()).await;
                    match t {
                        Ok(c) => {
                            debug!("rev grpc server endpoint connected");
                            channel = c;
                            break;
                        }
                        Err(e) => {
                            trace!("rev grpc server endpoint connect error {:?}", e);
                            sleep(Duration::from_millis(300)).await;
                        }
                    }
                }
                let (mtx, mrx) = mpsc::channel::<ManagerReq>(8);
                let mrx = tokio_stream::wrappers::ReceiverStream::new(mrx);
                let mrx = Request::new(mrx);

                let mut client = RogReverseServiceClient::new(channel);

                if let Err(e) = mtx
                    .send(ManagerReq {
                        auth: auth.clone(),
                        tag: tag.clone(),
                        addr_info: None,
                        udp: None,
                        conn_id: None,
                    })
                    .await
                {
                    error!("manager initial auth send error {}", e);
                    sleep(Duration::from_millis(300)).await;
                    continue;
                }

                let manager_stream = client.manager(mrx).await;
                match manager_stream {
                    Ok(manager_stream) => {
                        debug!("rev grpc manager stream created");
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
                                            debug!("manager manager stream got req {:?}", req);
                                            if handle_manage_req(req, &mut client, &tx, &utx, &auth).await.is_err() {
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
        }))
    }
}

#[async_trait::async_trait]
impl RunAcceptor for RevGrpcRunListener {
    async fn accept(&self) -> std::io::Result<(RunAccStream, SocketAddr)> {
        let mut receiver = self.receiver.lock().await;
        let mut udp_receiver = self.udp_receiver.lock().await;
        select! {
            r = udp_receiver.recv() => {
                match r {
                    None => Err(Error::other("udp receiver closed")),
                    Some((reader, writer)) => Ok((
                        RunAccStream::UDPSocket((reader, writer)),
                        "127.0.9.28:2809".parse().unwrap(),
                    )),
                }
            }
            r = receiver.recv() => {
                match r {
                    None => Err(Error::other("receiver closed")),
                    Some(stream) => Ok((RunAccStream::TCPStream(Box::new(stream)), "127.0.0.1:2809".parse().unwrap())),
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
