use std::io::{Error, ErrorKind};
use std::net::SocketAddr;
use std::pin::Pin;
use std::sync::Arc;
use futures::{Stream, StreamExt};
use tokio::spawn;
use tokio::sync::{mpsc, Mutex};
use tokio::sync::mpsc::{Receiver, Sender};
use tonic::{Code, Request, Response, Status, Streaming};
use tonic::codegen::tokio_stream::wrappers::ReceiverStream;
use tonic::transport::Server;
use crate::def::{RunAcceptor, RunListener, RunReadHalf, RunStream, RunWriteHalf};
use crate::stream::grpc_client::pb::rog_service_server::{RogService, RogServiceServer};
use crate::stream::grpc_client::pb::{StreamReq, StreamRes, UdpReq, UdpRes};
use crate::stream::grpc_server::GrpcServerRunStream;
use crate::util::RunAddr;

pub struct GrpcListener {
    auth: String,
}

impl GrpcListener {
    pub fn new(auth: String) -> GrpcListener {
        GrpcListener { auth }
    }
}

struct GrpcServer {
    sender: Sender<GrpcServerRunStream>,
    auth: String,
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
        todo!()
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
        let rog = GrpcServer { sender: tx, auth: self.auth.clone() };
        let addr = addr.to_owned();
        spawn(async move {
            let _ = Server::builder()
                .add_service(RogServiceServer::new(rog))
                .serve(addr.parse().unwrap()).await;
        });
        Ok(Box::new(GrpcRunListener {
            receiver: Arc::new(Mutex::new(rx)),
            auth: self.auth.clone(),
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