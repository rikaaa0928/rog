use crate::def::{config, RunConnector, RunStream, RunUdpReader, RunUdpWriter};
use crate::proto::v1::pb::rog_service_client::RogServiceClient;
use crate::proto::v1::pb::{StreamReq, UdpReq};
use crate::stream::grpc_client::GrpcClientRunStream;
use crate::stream::grpc_udp_client::{GrpcUdpClientRunReader, GrpcUdpClientRunWriter};
use log::{error, info};
use std::io;
use std::io::ErrorKind;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::{mpsc, Mutex};
use tokio::time::sleep;
use tonic::codegen::tokio_stream;
use tonic::Request;
// pub mod pb {
//     tonic::include_proto!("moe.rikaaa0928.rog");
// }

pub struct GrpcRunConnector {
    client: Arc<Mutex<RogServiceClient<tonic::transport::Channel>>>,
    cfg: config::Connector,
}
impl GrpcRunConnector {
    pub async fn new(cfg: &config::Connector) -> io::Result<Self> {
        let endpoint = cfg.endpoint.as_ref().ok_or_else(|| {
            let err_msg = "gRPC connector config is missing 'endpoint'";
            error!("{}", err_msg);
            io::Error::new(ErrorKind::InvalidInput, err_msg)
        })?;
        let mut err = None;
        for i in 1..=3 {
            let client = RogServiceClient::connect(endpoint.clone()).await;
            match client {
                Ok(client) => {
                    if err.is_some() {
                        info!("grpc Connector {} is established after retry", endpoint);
                    }
                    return Ok(Self {
                        client: Arc::new(Mutex::new(client)),
                        cfg: cfg.clone(),
                    });
                }
                Err(e) => {
                    error!("grpc connection error: {}", e);
                    err = Some(e);
                    sleep(Duration::from_millis(i * 100)).await;
                }
            }
        }
        Err(io::Error::other(err.unwrap()))
    }
}

#[async_trait::async_trait]
impl RunConnector for GrpcRunConnector {
    async fn connect(&self, addr: String) -> io::Result<Box<dyn RunStream>> {
        let (host, port) = parse_address(addr.as_str())?;
        let (tx, rx) = mpsc::channel::<StreamReq>(8);
        let rx = tokio_stream::wrappers::ReceiverStream::new(rx);
        let rx = Request::new(rx);

        let resp = match self.client.lock().await.stream(rx).await {
            Ok(r) => r.into_inner(),
            Err(e) => {
                error!("gRPC connector failed to open stream: {}", e);
                return Err(io::Error::other(e));
            }
        };

        let t = tx.clone();
        let auth = StreamReq { auth: self.cfg.pw.clone().unwrap(), dst_port: Some(port as u32), dst_addr: Some(host), ..Default::default() };
        let res = t.send(auth).await;
        if let Err(e) = res {
            error!("gRPC connector failed to send auth request: {}", e);
            return Err(io::Error::other(
                "failed to send auth request",
            ));
        }

        let mut stream = GrpcClientRunStream::new(Arc::new(Mutex::new(resp)), tx);
        stream.set_info(&mut |x| x.protocol_name = "grpc".to_string());
        Ok(Box::new(stream))
    }

    async fn udp_tunnel(
        &self,
        src_addr: String,
    ) -> io::Result<Option<(Box<dyn RunUdpReader>, Box<dyn RunUdpWriter>)>> {
        let (tx, rx) = mpsc::channel::<UdpReq>(8);
        let rx = tokio_stream::wrappers::ReceiverStream::new(rx);
        let rx = Request::new(rx);
        let res = self.client.lock().await.udp(rx).await;
        if res.is_err() {
            return Err(io::Error::other("grpc stream error"));
        }
        let resp = res.unwrap().into_inner();
        Ok(Some((
            Box::new(GrpcUdpClientRunReader::new(
                resp,
                src_addr.clone(),
                self.cfg.pw.as_ref().unwrap().clone(),
            )),
            Box::new(GrpcUdpClientRunWriter::new(
                tx,
                src_addr,
                self.cfg.pw.as_ref().unwrap().clone(),
            )),
        )))
    }
}

fn parse_address(addr: &str) -> io::Result<(String, u16)> {
    // 使用 rsplit_once 从右边分割,这样可以处理 IPv6 地址中的冒号
    let (host, port) = addr
        .rsplit_once(':')
        .ok_or(io::Error::new(ErrorKind::InvalidData, "addr error"))?;

    // 解析端口
    let port: u16 = port
        .parse()
        .map_err(|_| io::Error::new(ErrorKind::InvalidData, "addr port error"))?;

    // 验证主机名不为空
    if host.is_empty() {
        return Err(io::Error::new(ErrorKind::InvalidData, "host empty error"));
    }

    Ok((host.to_string(), port))
}
