use crate::def::{ReadWrite, RunConnector, RunUdpReader, RunUdpWriter};
use crate::stream::udp::UdpRunStream;
use log::error;
use std::io::Result;
use std::sync::Arc;
use tokio::net::{TcpStream, UdpSocket};

pub struct TcpRunConnector {}

impl TcpRunConnector {
    pub fn new() -> Self {
        TcpRunConnector {}
    }
}

#[async_trait::async_trait]
impl RunConnector for TcpRunConnector {
    async fn connect(&self, addr: String) -> Result<Box<dyn ReadWrite>> {
        let tcp_stream = match TcpStream::connect(addr.clone()).await {
            Ok(s) => s,
            Err(e) => {
                error!("Tcp connector failed to connect to {}: {}", addr, e);
                return Err(e);
            }
        };
        Ok(Box::new(tcp_stream))
    }

    async fn udp_tunnel(
        &self,
        src_addr: String,
    ) -> Result<Option<(Box<dyn RunUdpReader>, Box<dyn RunUdpWriter>)>> {
        let inner = Arc::new(UdpSocket::bind("0.0.0.0:0").await?);
        Ok(Some((
            Box::new(UdpRunStream::new(inner.clone(), src_addr.clone())),
            Box::new(UdpRunStream::new(inner, src_addr)),
        )))
    }
}