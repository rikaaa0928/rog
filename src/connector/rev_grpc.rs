use crate::connector::grpc::parse_address;
use crate::def::config::Connector;
use crate::def::{RunConnector, RunStream, RunUdpReader, RunUdpWriter, config};
use crate::proto::v1::pb::rog_reverse_service_server::{
    RogReverseService, RogReverseServiceServer,
};
use crate::proto::v1::pb::{
    AddrInfo, ManagerReq, ManagerRes, RevStreamReq, RevStreamRes, RevUdpReq, RevUdpRes,
};
use crate::stream::rev_grpc_server::RevGrpcServerRunStream;
use dashmap::DashMap;
use futures::Stream;
use futures::StreamExt;
use log::{error, info, warn};
use std::io;
use std::io::ErrorKind;
use std::pin::Pin;
use std::sync::Arc;
use std::time::Duration;
use tokio::spawn;
use tokio::sync::{Mutex, mpsc, oneshot};
use tokio::time::timeout;
use tokio_stream::wrappers::ReceiverStream;
use tonic::transport::Server;
use tonic::{Request, Response, Status, Streaming};

// The state shared between the gRPC Server handlers and the RunConnector
pub struct PendingConn {
    pub tx: oneshot::Sender<RevGrpcServerRunStream>,
    pub host: String,
    pub port: u16,
}

pub struct RevGrpcState {
    pub managers: DashMap<String, mpsc::Sender<Result<ManagerRes, Status>>>,
    pub pending_streams: DashMap<String, PendingConn>,
}

impl RevGrpcState {
    pub fn new() -> Self {
        Self {
            managers: DashMap::new(),
            pending_streams: DashMap::new(),
        }
    }
}

pub struct RevGrpcRunConnector {
    cfg: config::Connector,
    state: Arc<RevGrpcState>,
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

        let state = Arc::new(RevGrpcState::new());
        let rog = RevGrpcServer {
            cfg: cfg.clone(),
            state: state.clone(),
        };

        spawn(async move {
            let _ = Server::builder()
                .add_service(RogReverseServiceServer::new(rog))
                .serve(endpoint.parse().unwrap())
                .await;
        });

        Ok(Self {
            cfg: cfg.clone(),
            state,
        })
    }
}

#[async_trait::async_trait]
impl RunConnector for RevGrpcRunConnector {
    async fn connect(&self, addr: String) -> io::Result<Box<dyn RunStream>> {
        let (host, port) = parse_address(addr.as_str())?;

        let manager_tx = {
            if self.state.managers.is_empty() {
                return Err(io::Error::new(
                    ErrorKind::NotConnected,
                    "no reverse server connected",
                ));
            }
            self.state
                .managers
                .iter()
                .next()
                .map(|m| m.value().clone())
                .unwrap()
        };

        let conn_id = uuid::Uuid::new_v4().to_string();
        let (tx, rx) = oneshot::channel();

        self.state.pending_streams.insert(
            conn_id.clone(),
            PendingConn {
                tx,
                host: host.clone(),
                port: port as u16,
            },
        );

        let req = ManagerRes {
            tag: "".to_string(), // Tag can be ignored by Server, or filled if needed
            addr_info: Some(AddrInfo {
                dst_addr: host,
                dst_port: port as u32,
                src_addr: "".to_string(),
                src_port: 0,
            }),
            udp: Some(0),
            conn_id: Some(conn_id.clone()),
        };

        if let Err(e) = manager_tx.send(Ok(req)).await {
            self.state.pending_streams.remove(&conn_id);
            return Err(io::Error::new(
                ErrorKind::ConnectionAborted,
                format!("failed to send to manager: {}", e),
            ));
        }

        match timeout(Duration::from_secs(10), rx).await {
            Ok(Ok(stream)) => Ok(Box::new(stream)),
            Ok(Err(_)) => {
                self.state.pending_streams.remove(&conn_id);
                Err(io::Error::new(
                    ErrorKind::ConnectionAborted,
                    "stream channel closed",
                ))
            }
            Err(_) => {
                self.state.pending_streams.remove(&conn_id);
                Err(io::Error::new(
                    ErrorKind::TimedOut,
                    "timeout waiting for reverse stream",
                ))
            }
        }
    }

    async fn udp_tunnel(
        &self,
        _src_addr: String,
    ) -> io::Result<Option<(Box<dyn RunUdpReader>, Box<dyn RunUdpWriter>)>> {
        Err(io::Error::new(
            ErrorKind::Unsupported,
            "udp not implemented yet",
        ))
    }
}

struct RevGrpcServer {
    cfg: Connector,
    state: Arc<RevGrpcState>,
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
        let mut in_stream = request.into_inner();

        let first_msg = match in_stream.message().await {
            Ok(Some(msg)) => msg,
            Ok(None) => {
                warn!("grpc rev client manager missing first auth message");
                return Err(Status::invalid_argument("missing first auth message"));
            }
            Err(e) => {
                warn!("grpc rev client manager first_msg: {}", e);
                return Err(e);
            }
        };

        if let Some(cfg_pw) = &self.cfg.pw {
            if first_msg.auth != *cfg_pw {
                return Err(Status::unauthenticated("invalid auth"));
            }
        }

        let tag = first_msg.tag.clone();

        if !self.state.managers.is_empty() {
            return Err(Status::already_exists(
                "another server is already connected",
            ));
        }

        let (tx, rx) = mpsc::channel(32);
        self.state.managers.insert(tag.clone(), tx);
        info!("server with tag {} connected", tag);

        let state_clone = self.state.clone();
        let tag_clone = tag.clone();
        spawn(async move {
            while let Ok(Some(_)) = in_stream.message().await {}
            info!("server with tag {} disconnected", tag_clone);
            state_clone.managers.remove(&tag_clone);
        });

        let out_stream = ReceiverStream::new(rx);
        Ok(Response::new(Box::pin(out_stream) as Self::managerStream))
    }

    type streamStream = ResponseStream;

    async fn stream(
        &self,
        request: Request<Streaming<RevStreamReq>>,
    ) -> Result<Response<Self::streamStream>, Status> {
        let mut in_stream = request.into_inner();

        let first_msg = match in_stream.message().await {
            Ok(Some(msg)) => msg,
            Ok(None) => return Err(Status::invalid_argument("missing first stream message")),
            Err(e) => return Err(e),
        };

        if let Some(cfg_pw) = &self.cfg.pw {
            if first_msg.auth != *cfg_pw {
                return Err(Status::unauthenticated("invalid auth"));
            }
        }

        let conn_id = first_msg.conn_id.unwrap_or_default();
        if conn_id.is_empty() {
            return Err(Status::invalid_argument("missing conn_id"));
        }

        if let Some((_, pending)) = self.state.pending_streams.remove(&conn_id) {
            let (tx, rx) = mpsc::channel::<Result<RevStreamRes, Status>>(32);
            let out_stream = ReceiverStream::new(rx);

            // Create RunStream wrapper
            let run_stream = RevGrpcServerRunStream::new(
                Arc::new(Mutex::new(in_stream)),
                tx,
                pending.host,
                pending.port,
            );

            if pending.tx.send(run_stream).is_err() {
                warn!("failed to send connected stream to connector");
            }
            Ok(Response::new(Box::pin(out_stream) as Self::streamStream))
        } else {
            Err(Status::not_found("conn_id not found or expired"))
        }
    }

    type udpStream = ResponseUdp;

    async fn udp(
        &self,
        _request: Request<Streaming<RevUdpReq>>,
    ) -> Result<Response<Self::udpStream>, Status> {
        Err(Status::unimplemented("udp not implemented"))
    }
}
