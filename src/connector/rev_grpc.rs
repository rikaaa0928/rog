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
use std::collections::HashMap;
use std::sync::OnceLock;

// The state shared between the gRPC Server handlers and the RunConnector
pub struct PendingConn {
    pub tx: oneshot::Sender<RevGrpcServerRunStream>,
    pub host: String,
    pub port: u16,
    pub pw: Option<String>,
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

static REV_GRPC_STATE: OnceLock<Arc<RevGrpcState>> = OnceLock::new();

pub fn get_global_rev_grpc_state() -> Arc<RevGrpcState> {
    REV_GRPC_STATE.get_or_init(|| Arc::new(RevGrpcState::new())).clone()
}

pub async fn start_reverse_server(endpoint: String, pw_map: HashMap<String, Option<String>>) {
    let state = get_global_rev_grpc_state();
    let rog = RevGrpcServer { pw_map, state };

    spawn(async move {
        match Server::builder()
            .add_service(RogReverseServiceServer::new(rog))
            .serve(endpoint.parse().unwrap())
            .await
        {
            Ok(_) => info!("Reverse grpc server stopped"),
            Err(e) => error!("Reverse grpc server err: {}", e),
        }
    });
}

pub struct RevGrpcRunConnector {
    cfg: config::Connector,
    state: Arc<RevGrpcState>,
}

impl RevGrpcRunConnector {
    pub async fn new(cfg: &config::Connector) -> io::Result<Self> {
        let state = get_global_rev_grpc_state();

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
            let tag = &self.cfg.name;
            self.state
                .managers
                .get(tag)
                .map(|m| m.value().clone())
                .ok_or_else(|| {
                    io::Error::new(
                        ErrorKind::NotConnected,
                        format!("reverse client '{}' offline", tag),
                    )
                })?
        };

        let conn_id = uuid::Uuid::new_v4().to_string();
        let (tx, rx) = oneshot::channel();

        self.state.pending_streams.insert(
            conn_id.clone(),
            PendingConn {
                tx,
                host: host.clone(),
                port: port as u16,
                pw: self.cfg.pw.clone(),
            },
        );

        let req = ManagerRes {
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
    pw_map: HashMap<String, Option<String>>,
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

        if let Some(expected_pw) = self.pw_map.get(&first_msg.tag).and_then(|p| p.as_ref()) {
            if first_msg.auth != *expected_pw {
                return Err(Status::unauthenticated("invalid auth"));
            }
        }

        let tag = first_msg.tag.clone();

        if self.state.managers.contains_key(&tag) {
            return Err(Status::already_exists(
                format!("client with tag '{}' is already connected", tag),
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

        let conn_id = first_msg.conn_id.unwrap_or_default();
        if conn_id.is_empty() {
            return Err(Status::invalid_argument("missing conn_id"));
        }

        if let Some((_, pending)) = self.state.pending_streams.remove(&conn_id) {
            if let Some(expected_pw) = pending.pw.as_ref() {
                if first_msg.auth != *expected_pw {
                    return Err(Status::unauthenticated("invalid auth"));
                }
            }

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
