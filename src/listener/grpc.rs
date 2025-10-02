use crate::def::{ReadWrite, RunAccStream, RunAcceptor, RunListener};
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
use tokio::sync::mpsc::{self, Receiver, Sender};
use tokio::sync::Mutex;
use tokio::{select, spawn};
use tonic::codegen::tokio_stream::wrappers::ReceiverStream;
use tonic::transport::Server;
use tonic::{Code, Request, Response, Status, Streaming};

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
    udp_sender: Sender<(Streaming<UdpReq>, Sender<Result<UdpRes, Status>>, String)>,
    cfg: ObjectConfig,
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
        let (tx, rx) = mpsc::channel::<Result<StreamRes, Status>>(8);
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
                    Some(stream) => Ok((RunAccStream::TCPStream(Box::new(stream)), "127.0.0.1:2809".parse().unwrap())),
                }
            }
        }
    }

    async fn handshake(
        &self,
        stream: &mut (dyn ReadWrite + Unpin + Send),
    ) -> std::io::Result<(RunAddr, Option<Vec<u8>>)> {
        let grpc_stream = stream
            .as_any_mut()
            .downcast_mut::<GrpcServerRunStream>()
            .ok_or_else(|| Error::new(ErrorKind::Other, "not a grpc stream"))?;

        match grpc_stream.handshake().await? {
            Some((addr, auth)) => {
                if auth != self.auth {
                    return Err(Error::new(ErrorKind::Other, "invalid auth"));
                }
                Ok((addr, None))
            }
            None => Err(Error::new(ErrorKind::Other, "handshake failed")),
        }
    }

    async fn post_handshake(
        &self,
        _stream: &mut (dyn ReadWrite + Unpin + Send),
        _error: bool,
        _port: u16,
    ) -> std::io::Result<()> {
        Ok(())
    }
}