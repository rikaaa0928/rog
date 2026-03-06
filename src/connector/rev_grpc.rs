use crate::connector::grpc::parse_address;
use crate::def::config::Connector;
use crate::def::{RunConnector, RunStream, RunUdpReader, RunUdpWriter, config};
use crate::object::config::ObjectConfig;
use crate::proto::v1::pb::rog_reverse_service_client::RogReverseServiceClient;
use crate::proto::v1::pb::rog_reverse_service_server::{
    RogReverseService, RogReverseServiceServer,
};
use crate::proto::v1::pb::rog_service_client::RogServiceClient;
use crate::proto::v1::pb::rog_service_server::{RogService, RogServiceServer};
use crate::proto::v1::pb::{
    ManagerReq, ManagerRes, RevStreamReq, RevStreamRes, RevUdpReq, RevUdpRes, StreamReq, StreamRes,
    UdpReq, UdpRes,
};
use crate::stream::grpc_client::GrpcClientRunStream;
use crate::stream::grpc_server::GrpcServerRunStream;
use crate::stream::grpc_udp_client::{GrpcUdpClientRunReader, GrpcUdpClientRunWriter};
use crate::stream::rev_grpc_server::RevGrpcServerRunStream;
use futures::Stream;
use log::{error, info};
use std::io;
use std::io::ErrorKind;
use std::pin::Pin;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;
use tokio::spawn;
use tokio::sync::mpsc::Sender;
use tokio::sync::{Mutex, mpsc};
use tokio::time::sleep;
use tonic::codegen::tokio_stream;
use tonic::codegen::tokio_stream::StreamExt;
use tonic::transport::Server;
use tonic::{Request, Response, Status, Streaming};

pub struct RevGrpcRunConnector {
    cfg: config::Connector,
}
impl RevGrpcRunConnector {
    pub async fn new(cfg: &config::Connector) -> io::Result<Self> {
        let endpoint = cfg
            .endpoint
            .as_ref()
            .ok_or_else(|| {
                let err_msg = "gRPC connector config is missing 'endpoint'";
                error!("{}", err_msg);
                io::Error::new(ErrorKind::InvalidInput, err_msg)
            })?
            .to_owned();
        let rog = RevGrpcServer {
            cfg: cfg.clone(),
        };
        spawn(async move {
            let _ = Server::builder()
                .add_service(RogReverseServiceServer::new(rog))
                .serve(endpoint.parse().unwrap())
                .await;
        });
        Ok(Self { cfg: cfg.clone() })
    }
}

#[async_trait::async_trait]
impl RunConnector for RevGrpcRunConnector {
    async fn connect(&self, addr: String) -> io::Result<Box<dyn RunStream>> {
        let (host, port) = parse_address(addr.as_str())?;
        let (tx, rx) = mpsc::channel::<RevStreamReq>(8);
        let rx = tokio_stream::wrappers::ReceiverStream::new(rx);
        let rx = Request::new(rx);

        todo!()
    }

    async fn udp_tunnel(
        &self,
        src_addr: String,
    ) -> io::Result<Option<(Box<dyn RunUdpReader>, Box<dyn RunUdpWriter>)>> {
        let (tx, rx) = mpsc::channel::<RevUdpReq>(8);
        let rx = tokio_stream::wrappers::ReceiverStream::new(rx);
        let rx = Request::new(rx);
        todo!()
    }
}
struct RevGrpcServer {
    cfg: Connector,
    // router: Arc<dyn RouterSet>, // Removed
}
type ResponseStream = Pin<Box<dyn Stream<Item = Result<RevStreamRes, Status>> + Send>>;
type ResponseUdp = Pin<Box<dyn Stream<Item = Result<RevUdpRes, Status>> + Send>>;
type ResponseManager = Pin<Box<dyn Stream<Item = Result<ManagerRes, Status>> + Send>>;

#[tonic::async_trait]
impl RogReverseService for RevGrpcServer {
    type managerStream = ResponseManager;

    async fn manager(
        &self,
        request: Request<Streaming<ManagerReq>>,
    ) -> Result<Response<Self::managerStream>, Status> {

        todo!()
    }

    type streamStream = ResponseStream;

    async fn stream(
        &self,
        request: Request<Streaming<RevStreamReq>>,
    ) -> Result<Response<Self::streamStream>, Status> {
        todo!()
    }

    type udpStream = ResponseUdp;

    async fn udp(
        &self,
        request: Request<Streaming<RevUdpReq>>,
    ) -> Result<Response<Self::udpStream>, Status> {
        todo!()
    }
}
