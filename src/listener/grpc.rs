use crate::def::{RouterSet, RunAccStream, RunAcceptor, RunListener, RunReadHalf, RunWriteHalf};
use crate::object::config::ObjectConfig;
use crate::proto::v1::pb::rog_service_server::{RogService, RogServiceServer};
use crate::proto::v1::pb::{StreamReq, StreamRes, UdpReq, UdpRes};
use crate::stream::grpc_server::GrpcServerRunStream;
use crate::stream::grpc_udp_server::{GrpcUdpServerReadHalf, GrpcUdpServerWriteHalf};
use crate::util::RunAddr;
use futures::Stream;
use std::io::{Error, ErrorKind};
use std::net::SocketAddr;
use std::pin::Pin;
use std::sync::Arc;
use tokio::sync::mpsc::{Receiver, Sender};
use tokio::sync::{mpsc, Mutex};
use tokio::{select, spawn};
use tonic::codegen::tokio_stream::wrappers::ReceiverStream;
use tonic::transport::Server;
use tonic::{Code, Request, Response, Status, Streaming};

pub struct GrpcListener {
    cfg: ObjectConfig,
    // router: Arc<dyn RouterSet>, // Removed
}

impl GrpcListener {
    pub fn new(cfg: ObjectConfig) -> GrpcListener {
        // _router argument completely removed
        GrpcListener { cfg }
    }
}

struct GrpcServer {
    sender: Sender<GrpcServerRunStream>,
    udp_sender: Sender<(Streaming<UdpReq>, Sender<Result<UdpRes, Status>>, String)>,
    cfg: ObjectConfig,
    // router: Arc<dyn RouterSet>, // Removed
}
type ResponseStream = Pin<Box<dyn Stream<Item = Result<StreamRes, Status>> + Send>>;
type ResponseUdp = Pin<Box<dyn Stream<Item = Result<UdpRes, Status>> + Send>>;

#[tonic::async_trait]
impl RogService for GrpcServer {
    type streamStream = ResponseStream;

    async fn stream(
        &self,
        request: Request<Streaming<StreamReq>>,
    ) -> Result<Response<Self::streamStream>, Status> {
        let (tx, rx) = mpsc::channel(8);
        let request = request.into_inner();
        let stream = GrpcServerRunStream::new(Arc::new(Mutex::new(request)), tx);
        match self.sender.send(stream).await {
            Ok(_) => {}
            Err(err) => return Err(Status::new(Code::Internal, format!("{}", err))),
        }
        Ok(Response::new(Box::pin(ReceiverStream::new(rx))))
    }

    type udpStream = ResponseUdp;

    async fn udp(
        &self,
        request: Request<Streaming<UdpReq>>,
    ) -> Result<Response<Self::udpStream>, Status> {
        let (tx, rx) = mpsc::channel::<Result<UdpRes, Status>>(8);
        let request = request.into_inner();
        let stream = (request, tx, self.cfg.listener.pw.clone().unwrap());
        match self.udp_sender.send(stream).await {
            Ok(_) => {}
            Err(err) => return Err(Status::new(Code::Internal, format!("{}", err))),
        }
        // // todo remove
        // let router = self.router.clone();
        // let cfg = self.cfg.clone();
        // let name = cfg.listener.name.clone();
        // let r_name = cfg.listener.router.clone();
        // let _: tokio::task::JoinHandle<io::Result<()>> = spawn(async move {
        //     let (reader_interrupter, reader_interrupt_receiver) = oneshot::channel();
        //     let (writer_interrupter, mut writer_interrupt_receiver) = oneshot::channel();
        //     let mut udp_tunnel = None;
        //     let request = &mut request.into_inner();
        //     let writer_interrupter = Arc::new(Mutex::new(Some(writer_interrupter)));
        //     let reader_interrupt_receiver = Arc::new(Mutex::new(Some(reader_interrupt_receiver)));
        //     debug!("grpc udp read loop start");
        //     let tx = Arc::new(tx);
        //     loop {
        //         let tx = Arc::clone(&tx);
        //         let writer_interrupt_receiver = &mut writer_interrupt_receiver;
        //         let writer_interrupter = Arc::clone(&writer_interrupter);
        //
        //         let reader_interrupt_receiver = Arc::clone(&reader_interrupt_receiver);
        //         let req: Option<Result<UdpReq, Status>> = select! {
        //             t=request.next()=>{t}
        //             _=writer_interrupt_receiver=>{None}
        //         };
        //         match req {
        //             None => {
        //                 debug!("grpc udp read none");
        //                 break;
        //             }
        //             Some(Ok(udp_req)) => {
        //                 if udp_tunnel.is_none() {
        //                     let dst_addr = RunAddr {
        //                         addr: udp_req.dst_addr.as_ref().unwrap().clone(),
        //                         port: udp_req.dst_port.as_ref().unwrap().clone() as u16,
        //                         udp: true,
        //                         cache: None,
        //                     };
        //                     let client_name = router
        //                         .route(name.as_str(), r_name.as_str(), &dst_addr)
        //                         .await;
        //                     let conn_conf = cfg.connector.get(client_name.as_str()).unwrap();
        //                     let connector = connector::create(conn_conf).await?;
        //                     let udp_t = connector
        //                         .udp_tunnel(format!(
        //                             "{}:{}",
        //                             udp_req.src_addr.as_ref().unwrap(),
        //                             udp_req.src_port.as_ref().unwrap()
        //                         ))
        //                         .await?;
        //                     match udp_t {
        //                         Some(tunnel) => {
        //                             let arc_tunnel = Arc::new(Mutex::new(tunnel));
        //                             udp_tunnel = Some(Arc::clone(&arc_tunnel));
        //                             let _: tokio::task::JoinHandle<io::Result<()>> = spawn(
        //                                 async move {
        //                                     let udp_tunnel = Arc::clone(&arc_tunnel);
        //                                     debug!("grpc udp write loop start");
        //                                     let mut reader_interrupt_receiver =
        //                                         Arc::clone(&reader_interrupt_receiver);
        //                                     loop {
        //                                         let mut reader_interrupt_receiver =
        //                                             reader_interrupt_receiver.lock().await;
        //                                         let udp_tunnel = udp_tunnel.lock().await;
        //                                         let res: io::Result<UDPPacket> = select! {
        //                                             t=udp_tunnel.read()=>{t}
        //                                             _=reader_interrupt_receiver.as_mut().unwrap()=>{
        //                                                 Err(Error::new(ErrorKind::Interrupted,"interrupt_receiver"))
        //                                             }
        //                                         };
        //                                         match res {
        //                                             Ok(udp_packet) => {
        //                                                 let res = UdpRes {
        //                                                     payload: udp_packet.data,
        //                                                     dst_addr: Some(
        //                                                         udp_packet.meta.dst_addr,
        //                                                     ),
        //                                                     dst_port: Some(
        //                                                         udp_packet.meta.dst_port as u32,
        //                                                     ),
        //                                                     src_addr: Some(
        //                                                         udp_packet.meta.src_addr,
        //                                                     ),
        //                                                     src_port: Some(
        //                                                         udp_packet.meta.src_port as u32,
        //                                                     ),
        //                                                 };
        //                                                 let res = tx.send(Ok(res)).await;
        //                                                 if res.is_err() {
        //                                                     debug!("grpc udp write error");
        //                                                     break;
        //                                                 }
        //                                             }
        //                                             Err(e) => {
        //                                                 debug!("grpc udp write loop read interrupted {:?}",e);
        //                                                 break;
        //                                             }
        //                                         }
        //                                     }
        //                                     match writer_interrupter.lock().await.take() {
        //                                         None => {}
        //                                         Some(i) => {
        //                                             let _ = i.send(());
        //                                         }
        //                                     }
        //                                     debug!("grpc udp write loop end");
        //                                     Ok(())
        //                                 },
        //                             );
        //                         }
        //                         None => {
        //                             break;
        //                         }
        //                     }
        //                 }
        //                 let udp_tunnel = udp_tunnel.as_ref().unwrap();
        //                 let udp_packet = UDPPacket {
        //                     meta: UDPMeta {
        //                         dst_addr: udp_req.dst_addr.as_ref().unwrap().clone(),
        //                         dst_port: udp_req.dst_port.as_ref().unwrap().clone() as u16,
        //                         src_addr: udp_req.src_addr.as_ref().unwrap().clone(),
        //                         src_port: udp_req.src_port.as_ref().unwrap().clone() as u16,
        //                     },
        //                     data: udp_req.payload.unwrap(),
        //                 };
        //                 let res = Arc::clone(udp_tunnel).lock().await.write(udp_packet).await;
        //                 if res.is_err() {
        //                     debug!("grpc udp read loop tunnel write error");
        //                     break;
        //                 }
        //             }
        //             Some(Err(err)) => {
        //                 debug!("grpc udp read loop read error");
        //                 break;
        //             }
        //         }
        //     }
        //     let _ = reader_interrupter.send(());
        //     debug!("grpc udp read loop end");
        //     Ok(())
        // });

        // todo remove above
        Ok(Response::new(Box::pin(ReceiverStream::new(rx))))
    }
}

pub struct GrpcRunListener {
    receiver: Arc<Mutex<Receiver<GrpcServerRunStream>>>,
    udp_receiver: Arc<Mutex<Receiver<(Streaming<UdpReq>, Sender<Result<UdpRes, Status>>, String)>>>,
    auth: String,
}

#[async_trait::async_trait]
impl RunListener for GrpcListener {
    async fn listen(&self, addr: &str) -> std::io::Result<Box<dyn RunAcceptor>> {
        let (tx, rx) = mpsc::channel(8);
        let (udp_tx, udp_rx) = mpsc::channel(8);
        let rog = GrpcServer {
            sender: tx,
            udp_sender: udp_tx,
            cfg: self.cfg.clone(),
            // router: self.router.clone(), // Removed
        };
        let addr = addr.to_owned();
        spawn(async move {
            let _ = Server::builder()
                .add_service(RogServiceServer::new(rog))
                .serve(addr.parse().unwrap())
                .await;
        });
        Ok(Box::new(GrpcRunListener {
            receiver: Arc::new(Mutex::new(rx)),
            udp_receiver: Arc::new(Mutex::new(udp_rx)),
            auth: self.cfg.listener.pw.as_ref().unwrap().clone(),
        }))
    }
}

#[async_trait::async_trait]
impl RunAcceptor for GrpcRunListener {
    async fn accept(&self) -> std::io::Result<(RunAccStream, SocketAddr)> {
        let mut receiver = self.receiver.lock().await;
        let mut udp_receiver = self.udp_receiver.lock().await;
        select! {
            r = udp_receiver.recv() => {
                match r {
                    None => Err(Error::new(ErrorKind::Other, "receiver closed")),
                    Some((r,w,a)) => Ok(((RunAccStream::UDPSocket(
                        (
                            Box::new(GrpcUdpServerReadHalf::new(r,a)),
                            Box::new(GrpcUdpServerWriteHalf::new(w))
                            )
                    )), "127.0.9.28:2809".parse().unwrap())),
                }
            }
            r = receiver.recv() => {
                match r {
                    None => Err(Error::new(ErrorKind::Other, "receiver closed")),
                    Some(stream) => Ok((RunAccStream::TCPStream( Box::new(stream)), "127.0.0.1:2809".parse().unwrap())),
                }
            }
        }
    }

    async fn handshake(
        &self,
        r: &mut dyn RunReadHalf,
        w: &mut dyn RunWriteHalf,
    ) -> std::io::Result<(RunAddr, Option<Vec<u8>>)> {
        match r.handshake().await? {
            Some((addr, auth)) => {
                if auth != self.auth {
                    return Err(Error::new(ErrorKind::Other, "invalid auth"));
                }
                Ok((addr, None))
            }
            None => Err(Error::new(ErrorKind::Other, "handshake failed")),
        }
    }
}
