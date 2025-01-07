use std::io::{Result};
use tokio::net::{TcpStream, UdpSocket};
use crate::def::{RunConnector, RunStream, RunUdpStream};
use crate::stream::tcp::TcpRunStream;
use crate::stream::udp::UdpRunStream;

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

    async fn udp_tunnel(&self, src_addr: String) -> Result<Option<Box<dyn RunUdpStream>>> {
        let inner = UdpSocket::bind("0.0.0.0:0").await?;
        Ok(Some(Box::new(UdpRunStream::new(inner, src_addr))))
    }
}