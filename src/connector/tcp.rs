use std::io::{Error, Result};
// use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpStream, UdpSocket};
use std::pin::Pin;
use std::future::Future;
// use std::net::SocketAddr;
use std::sync::Arc;
use tokio::sync::Mutex;
use crate::def::{RunConnector, RunReadHalf, RunStream, RunUdpStream, RunWriteHalf};
use crate::stream::tcp::TcpRunStream;
use crate::stream::udp::UdpRunStream;

pub struct TcpRunConnector {}

impl TcpRunConnector {
    pub fn new() -> Self {
        TcpRunConnector {}
    }
}

// #[async_trait::async_trait]
// impl RunUdpConnector for TcpRunConnector {
//     async fn udp_tunnel(&self, src_addr: String) -> Result<Option<Box<dyn RunUdpStream>>> {
//         let inner = UdpSocket::bind("0.0.0.0:0").await?;
//         Ok(Some(Box::new(UdpRunStream::new(inner, src_addr))))
//     }
// }

#[async_trait::async_trait]
impl RunConnector for Arc<Mutex<TcpRunConnector>> {
    async fn connect(&self, addr: String) -> Result<Box<dyn RunStream>> {
        let connector = self.clone();
        let connector = connector.lock().await;
        connector.connect(addr).await
    }

    async fn udp_tunnel(&self, src_addr: String) -> Result<Option<Box<dyn RunUdpStream>>> {
        let connector = self.clone();
        let connector = connector.lock().await;
        connector.udp_tunnel(src_addr).await
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