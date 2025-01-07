use std::io;
use std::io::ErrorKind;
use std::sync::Arc;
use tokio::sync::{mpsc, Mutex};
use tonic::codegen::tokio_stream;
use tonic::{Request};
use crate::stream::grpc_client::pb::{StreamReq, UdpReq};
use crate::stream::grpc_client::pb::rog_service_client::RogServiceClient;
use crate::def::{config, RunConnector, RunStream, RunUdpStream};
use crate::stream::grpc_client::GrpcClientRunStream;
use crate::stream::grpc_udp_client::GrpcUdpClientRunStream;
// pub mod pb {
//     tonic::include_proto!("moe.rikaaa0928.rog");
// }

pub struct GrpcRunConnector {
    client: Arc<Mutex<RogServiceClient<tonic::transport::Channel>>>,
    cfg: config::Connector,
}
impl GrpcRunConnector {
    pub async fn new(cfg: &config::Connector) -> io::Result<Self> {
        let x = RogServiceClient::connect(cfg.endpoint.as_ref().unwrap().clone()).await;
        if x.is_err() {
            return Err(io::Error::new(ErrorKind::InvalidData, "invalid endpoint"));
        }
        let client = x.unwrap();
        Ok(Self {
            client: Arc::new(Mutex::new(client)),
            cfg: cfg.clone(),
        })
    }
}

#[async_trait::async_trait]
impl RunConnector for GrpcRunConnector {
    async fn connect(&self, addr: String) -> io::Result<Box<dyn RunStream>> {
        let (host, port) = parse_address(addr.as_str())?;
        let (tx, rx) = mpsc::channel::<StreamReq>(1024);
        let rx = tokio_stream::wrappers::ReceiverStream::new(rx);
        let rx = Request::new(rx);
        let res = self.client.lock().await.stream(rx).await;
        if res.is_err() {
            return Err(io::Error::new(ErrorKind::Other, "grpc stream error"));
        }
        let resp = res.unwrap().into_inner();
        let t = tx.clone();
        let mut auth = StreamReq::default();
        auth.auth = self.cfg.pw.clone().unwrap();
        auth.dst_port = Some(port as u32);
        auth.dst_addr = Some(host);
        let res = t.send(auth).await;
        if res.is_err() {
            return Err(io::Error::new(ErrorKind::Other, "failed to send auth"));
        }

        Ok(Box::new(GrpcClientRunStream::new(Arc::new(Mutex::new(resp)), tx)))
    }

    async fn udp_tunnel(&self, src_addr: String) -> io::Result<Option<Box<dyn RunUdpStream>>> {
        let (tx, rx) = mpsc::channel::<UdpReq>(1024);
        let rx = tokio_stream::wrappers::ReceiverStream::new(rx);
        let rx = Request::new(rx);
        let res = self.client.lock().await.udp(rx).await;
        if res.is_err() {
            return Err(io::Error::new(ErrorKind::Other, "grpc stream error"));
        }
        let resp = res.unwrap().into_inner();
        Ok(Some(Box::new(GrpcUdpClientRunStream::new(Arc::new(Mutex::new(resp)), tx,src_addr,self.cfg.pw.as_ref().unwrap().clone()))))
    }
}

fn parse_address(addr: &str) -> io::Result<(String, u16)> {
    // 使用 rsplit_once 从右边分割,这样可以处理 IPv6 地址中的冒号
    let (host, port) = addr.rsplit_once(':')
        .ok_or(io::Error::new(ErrorKind::InvalidData, "addr error"))?;

    // 解析端口
    let port: u16 = port.parse()
        .map_err(|_| io::Error::new(ErrorKind::InvalidData, "addr port error"))?;

    // 验证主机名不为空
    if host.is_empty() {
        return Err(io::Error::new(ErrorKind::InvalidData, "host empty error"));
    }

    Ok((host.to_string(), port))
}
