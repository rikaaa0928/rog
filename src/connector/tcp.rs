use crate::def::{RunConnector, RunStream, RunUdpReader, RunUdpWriter};
use crate::stream::tcp::TcpRunStream;
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
    async fn connect(&self, addr: String) -> Result<Box<dyn RunStream>> {
        let tcp_stream = match TcpStream::connect(addr.clone()).await {
            Ok(s) => s,
            Err(e) => {
                error!("Tcp connector failed to connect to {}: {}", addr, e);
                return Err(e);
            }
        };
        let mut stream = TcpRunStream::new(tcp_stream);
        stream.set_info(&mut |x| x.protocol_name = "tcp".to_string());
        Ok(Box::new(stream))
    }

    async fn udp_tunnel(
        &self,
        src_addr: String,
    ) -> Result<Option<(Box<dyn RunUdpReader>, Box<dyn RunUdpWriter>)>> {
        let socket = match socket2::Socket::new(
            socket2::Domain::IPV6,
            socket2::Type::DGRAM,
            Some(socket2::Protocol::UDP),
        )
        .and_then(|sock| {
            let _ = sock.set_only_v6(false);
            sock.set_nonblocking(true)?;
            let unspec: std::net::SocketAddr = "[::]:0".parse().unwrap();
            sock.bind(&unspec.into())?;
            UdpSocket::from_std(sock.into())
        }) {
            Ok(s) => s,
            Err(_) => UdpSocket::bind("0.0.0.0:0").await?,
        };
        let inner = Arc::new(socket);
        let (reader, writer) = UdpRunStream::new_pair(inner, src_addr);
        Ok(Some((Box::new(reader), Box::new(writer))))
    }
}
