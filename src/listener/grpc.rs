use std::io;
use std::io::{Error, ErrorKind};
use std::net::SocketAddr;
use std::pin::Pin;
use std::sync::Arc;
use futures::{Stream, StreamExt};
use log::debug;
use tokio::{select, spawn};
use tokio::sync::{mpsc, oneshot, Mutex};
use tokio::sync::mpsc::{Receiver, Sender};
use tonic::{Code, Request, Response, Status, Streaming};
use tonic::codegen::tokio_stream::wrappers::ReceiverStream;
use tonic::transport::Server;
use crate::connector;
use crate::def::{Router, RunAcceptor, RunListener, RunReadHalf, RunStream, RunWriteHalf, UDPMeta, UDPPacket};
use crate::object::config::ObjectConfig;
use crate::router::DefaultRouter;
use crate::stream::grpc_client::pb::rog_service_server::{RogService, RogServiceServer};
use crate::stream::grpc_client::pb::{StreamReq, StreamRes, UdpReq, UdpRes};
use crate::stream::grpc_server::GrpcServerRunStream;
use crate::util::RunAddr;

pub struct GrpcListener {
    cfg: ObjectConfig,
}

impl GrpcListener {
    pub fn new(cfg: ObjectConfig) -> GrpcListener {
        GrpcListener { cfg }
    }
}

struct GrpcServer {
    sender: Sender<GrpcServerRunStream>,
    cfg: ObjectConfig,
    router: DefaultRouter,
}
type ResponseStream = Pin<Box<dyn Stream<Item=Result<StreamRes, Status>> + Send>>;
type ResponseUdp = Pin<Box<dyn Stream<Item=Result<UdpRes, Status>> + Send>>;

#[tonic::async_trait]
impl RogService for GrpcServer {
    type streamStream = ResponseStream;

    async fn stream(&self, request: Request<Streaming<StreamReq>>) -> Result<Response<Self::streamStream>, Status> {
        let (tx, rx) = mpsc::channel(1024);
        let request = request.into_inner();
        let stream = GrpcServerRunStream::new(Arc::new(Mutex::new(request)), tx);
        match self.sender.send(stream).await {
            Ok(_) => {}
            Err(err) => {
                return Err(Status::new(Code::Internal, format!("{}", err)))
            }
        }
        Ok(Response::new(Box::pin(ReceiverStream::new(rx))))
    }

    type udpStream = ResponseUdp;

    async fn udp(&self, request: Request<Streaming<UdpReq>>) -> Result<Response<Self::udpStream>, Status> {
        let (tx, rx) = mpsc::channel::<Result<UdpRes, Status>>(1024);
        let router = self.router.clone();
        let cfg = self.cfg.clone();
        let _: tokio::task::JoinHandle<io::Result<()>> = spawn(async move {
            let (reader_interrupter, reader_interrupt_receiver) = oneshot::channel();
            let (writer_interrupter, mut writer_interrupt_receiver) = oneshot::channel();
            let mut udp_tunnel = None;
            let request = &mut request.into_inner();
            let writer_interrupter = Arc::new(Mutex::new(Some(writer_interrupter)));
            let reader_interrupt_receiver = Arc::new(Mutex::new(Some(reader_interrupt_receiver)));
            debug!("grpc udp read loop start");
            let tx = Arc::new(tx);
            loop {
                let tx = Arc::clone(&tx);
                let writer_interrupt_receiver = &mut writer_interrupt_receiver;
                let writer_interrupter = Arc::clone(&writer_interrupter);

                let reader_interrupt_receiver = Arc::clone(&reader_interrupt_receiver);
                let req: Option<Result<UdpReq, Status>> = select! { 
                    t=request.next()=>{t}
                    _=writer_interrupt_receiver=>{None}
                };
                match req {
                    None => {
                        debug!("grpc udp read none");
                        break;
                    }
                    Some(Ok(udp_req)) => {
                        if udp_tunnel.is_none() {
                            let dst_addr = RunAddr {
                                addr: udp_req.dst_addr.as_ref().unwrap().clone(),
                                port: udp_req.dst_port.as_ref().unwrap().clone() as u16,
                                udp: true,
                                cache: None,
                            };
                            let client_name = router.route(&dst_addr).await?;
                            let conn_conf = cfg.connector.get(client_name.as_str()).unwrap();
                            let connector = connector::create(conn_conf).await?;
                            let udp_t = connector.udp_tunnel(format!("{}:{}", udp_req.src_addr.as_ref().unwrap(), udp_req.src_port.as_ref().unwrap())).await?;
                            match udp_t {
                                Some(tunnel) => {
                                    let arc_tunnel = Arc::new(Mutex::new(tunnel));
                                    udp_tunnel = Some(Arc::clone(&arc_tunnel));
                                    let _: tokio::task::JoinHandle<io::Result<()>> = spawn(async move {
                                        let udp_tunnel = Arc::clone(&arc_tunnel);
                                        debug!("grpc udp write loop start");
                                        let mut reader_interrupt_receiver = Arc::clone(&reader_interrupt_receiver);
                                        loop {
                                            let mut reader_interrupt_receiver = reader_interrupt_receiver.lock().await;
                                            let udp_tunnel = udp_tunnel.lock().await;
                                            let res: io::Result<UDPPacket> = select! { 
                                                t=udp_tunnel.read()=>{t}
                                                _=reader_interrupt_receiver.as_mut().unwrap()=>{
                                                    Err(Error::new(ErrorKind::Interrupted,"interrupt_receiver"))
                                                }
                                            };
                                            match res {
                                                Ok(udp_packet) => {
                                                    let res = UdpRes {
                                                        payload: udp_packet.data,
                                                        dst_addr: Some(udp_packet.meta.dst_addr),
                                                        dst_port: Some(udp_packet.meta.dst_port as u32),
                                                        src_addr: Some(udp_packet.meta.src_addr),
                                                        src_port: Some(udp_packet.meta.src_port as u32),
                                                    };
                                                    let res = tx.send(Ok(res)).await;
                                                    if res.is_err() {
                                                        debug!("grpc udp write error");
                                                        break;
                                                    }
                                                }
                                                Err(e) => {
                                                    debug!("grpc udp write loop read interrupted {:?}",e);
                                                    break;
                                                }
                                            }
                                        }
                                        match writer_interrupter.lock().await.take() {
                                            None => {}
                                            Some(i) => {
                                                let _ = i.send(());
                                            }
                                        }
                                        debug!("grpc udp write loop end");
                                        Ok(())
                                    });
                                }
                                None => {
                                    break;
                                }
                            }
                        }
                        let udp_tunnel = udp_tunnel.as_ref().unwrap();
                        let udp_packet = UDPPacket {
                            meta: UDPMeta {
                                dst_addr: udp_req.dst_addr.as_ref().unwrap().clone(),
                                dst_port: udp_req.dst_port.as_ref().unwrap().clone() as u16,
                                src_addr: udp_req.src_addr.as_ref().unwrap().clone(),
                                src_port: udp_req.src_port.as_ref().unwrap().clone() as u16,
                            },
                            data: udp_req.payload.unwrap(),
                        };
                        let res = Arc::clone(udp_tunnel).lock().await.write(udp_packet).await;
                        if res.is_err() {
                            debug!("grpc udp read loop tunnel write error");
                            break;
                        }
                    }
                    Some(Err(err)) => {
                        debug!("grpc udp read loop read error");
                        break;
                    }
                }
            }
            let _ = reader_interrupter.send(());
            debug!("grpc udp read loop end");
            Ok(())
        });
        Ok(Response::new(Box::pin(ReceiverStream::new(rx))))
    }
}

pub struct GrpcRunListener {
    receiver: Arc<Mutex<Receiver<GrpcServerRunStream>>>,
    auth: String,
}

#[async_trait::async_trait]
impl RunListener for GrpcListener {
    async fn listen(&self, addr: &str) -> std::io::Result<Box<dyn RunAcceptor>> {
        let (tx, rx) = mpsc::channel(1024);
        let router = DefaultRouter::new(&self.cfg.router);
        let rog = GrpcServer { sender: tx, cfg: self.cfg.clone(), router };
        let addr = addr.to_owned();
        spawn(async move {
            let _ = Server::builder()
                .add_service(RogServiceServer::new(rog))
                .serve(addr.parse().unwrap()).await;
        });
        Ok(Box::new(GrpcRunListener {
            receiver: Arc::new(Mutex::new(rx)),
            auth: self.cfg.listener.pw.as_ref().unwrap().clone(),
        }))
    }
}

#[async_trait::async_trait]
impl RunAcceptor for GrpcRunListener {
    async fn accept(&self) -> std::io::Result<(Box<dyn RunStream>, SocketAddr)> {
        let mut receiver = self.receiver.lock().await;
        match receiver.recv().await {
            None => {
                Err(Error::new(ErrorKind::Other, "receiver closed"))
            }
            Some(stream) => {
                Ok(
                    (Box::new(
                        stream
                    )
                     , "127.0.0.1:2809".parse().unwrap())
                )
            }
        }
    }

    async fn handshake(&self, r: &mut dyn RunReadHalf, w: &mut dyn RunWriteHalf) -> std::io::Result<RunAddr> {
        match r.handshake().await? {
            Some((addr, auth)) => {
                if auth != self.auth {
                    return Err(Error::new(ErrorKind::Other, "invalid auth"));
                }
                Ok(addr)
            }
            None => {
                Err(Error::new(ErrorKind::Other, "handshake failed"))
            }
        }
    }

    async fn post_handshake(&self, r: &mut dyn RunReadHalf, w: &mut dyn RunWriteHalf, error: bool, port: u16) -> std::io::Result<()> {
        Ok(())
    }
}