use crate::def::{RunConnector, RunStream, RunUdpReader, RunUdpWriter};
use crate::stream::tcp::TcpRunStream;
use crate::stream::udp::UdpRunStream;
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
    async fn connect(&self, addr: String) -> Result<Box<dyn RunStream>> {
        let tcp_stream = TcpStream::connect(addr).await?;
        Ok(Box::new(TcpRunStream::new(tcp_stream)))
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
